//! wasmtime behind the [`Plugin`] trait.
//!
//! A [`WasmPlugin`] holds its compiled component; every call instantiates it afresh in a store
//! with the [`Limits`], so nothing a plugin did in one rebuild is there in the next. The store
//! lives on a worker thread for the call: the plugin's `host.run` crosses back over a channel
//! to the calling thread, which runs the command on the borrowed [`Host`] (a store's data must
//! own what it holds, and the host is a borrow of the scratch session).

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use serde_json::Value;
use solidcraft_plugin::{Host, Manifest, Plugin, Schema};
use wasmtime::component::{Component, ComponentExportIndex, Instance, InstancePre, Linker};
use wasmtime::{Config, Engine, ResourceLimiter, Store, StoreContextMut, Trap};

use crate::Limits;

/// The world's interfaces, by their full names (`crates/plugin/wit/plugin.wit`).
const HOST_INTERFACE: &str = "solidcraft:plugin/host@1.0.0";
const PLUGIN_INTERFACE: &str = "solidcraft:plugin/plugin@1.0.0";

/// One `host.run` from the plugin: the command, its JSON arguments, and where the answer goes.
type Request = (String, String, mpsc::Sender<Result<String, String>>);

/// A store's data: its memory limit and the way back to the calling thread's host.
struct State {
    limiter: Limiter,
    requests: Option<mpsc::Sender<Request>>,
}

impl State {
    fn run(&mut self, command: String, args: String) -> Result<String, String> {
        let Some(requests) = &self.requests else { return Err("the plugin cannot run commands here".into()) };
        let (reply, answer) = mpsc::channel();
        requests.send((command, args, reply)).map_err(|_| "the plugin cannot run commands here".to_string())?;
        answer.recv().unwrap_or_else(|_| Err("the host stopped answering".into()))
    }
}

/// Memory growth up to the limit; growth past it fails (the plugin sees an allocation failure)
/// and is remembered, so the failure is reported as memory and not as the trap that follows.
struct Limiter {
    memory_bytes: usize,
    refused: Option<usize>,
}

impl ResourceLimiter for Limiter {
    fn memory_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        if desired > self.memory_bytes {
            self.refused = Some(desired);
            return Ok(false);
        }
        Ok(true)
    }
    fn table_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        Ok(desired <= 1 << 20)
    }
}

/// Compiles plugin components with one engine and the limits their calls get.
pub struct Loader {
    engine: Engine,
    linker: Linker<State>,
    limits: Limits,
}

impl Loader {
    pub fn new(limits: Limits) -> Result<Loader, String> {
        let mut config = Config::new();
        config.wasm_component_model(true).consume_fuel(true).max_wasm_stack(limits.stack_bytes);
        let engine = Engine::new(&config).map_err(|e| format!("the WebAssembly engine could not start ({e})"))?;
        let mut linker = Linker::new(&engine);
        linker
            .instance(HOST_INTERFACE)
            .and_then(|mut host| {
                host.func_wrap("run", |mut cx: StoreContextMut<'_, State>, (command, args): (String, String)| Ok((cx.data_mut().run(command, args),)))
            })
            .map_err(|e| format!("the plugin host could not be set up ({e})"))?;
        Ok(Loader { engine, linker, limits })
    }

    /// Compile a component file and read its manifest and schema.
    pub fn load_file(&self, path: &Path) -> Result<WasmPlugin, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{} could not be read ({e})", path.display()))?;
        self.load(&bytes)
    }

    /// Compile a component and read its manifest and schema.
    pub fn load(&self, bytes: &[u8]) -> Result<WasmPlugin, String> {
        let component = Component::new(&self.engine, bytes).map_err(|e| format!("not a WebAssembly component ({})", first_line(&e)))?;
        let pre = self.linker.instantiate_pre(&component).map_err(|e| format!("the component needs more than `host.run` ({})", first_line(&e)))?;
        let interface =
            component.get_export_index(None, PLUGIN_INTERFACE).ok_or_else(|| format!("the component does not export `{PLUGIN_INTERFACE}`"))?;
        let export = |name: &str| component.get_export_index(Some(&interface), name).ok_or_else(|| format!("the component's plugin has no `{name}`"));
        let exports =
            Exports { manifest: export("manifest")?, schema: export("schema")?, evaluate: export("evaluate")?, describe: export("describe")? };
        let mut plugin = WasmPlugin {
            engine: self.engine.clone(),
            pre,
            exports,
            limits: self.limits,
            manifest: Manifest {
                id: "(loading)".into(),
                name: String::new(),
                version: String::new(),
                api_version: 0,
                description: String::new(),
                display: Default::default(),
                fallback: Default::default(),
                preview: false,
            },
            schema: Schema::default(),
            fuel_used: AtomicU64::new(0),
        };
        let manifest = plugin.text(None, Call::Manifest)?;
        plugin.manifest = serde_json::from_str(&manifest).map_err(|e| format!("the component's manifest is not a plugin manifest ({e})"))?;
        let schema = plugin.text(None, Call::Schema)?;
        plugin.schema = serde_json::from_str(&schema).map_err(|e| format!("the plugin `{}`'s schema could not be read ({e})", plugin.manifest.id))?;
        Ok(plugin)
    }
}

struct Exports {
    manifest: ComponentExportIndex,
    schema: ComponentExportIndex,
    evaluate: ComponentExportIndex,
    describe: ComponentExportIndex,
}

enum Call<'a> {
    Manifest,
    Schema,
    Evaluate(&'a str, &'a str),
    Describe(&'a str, &'a str),
}

/// A plugin loaded from a component. Its manifest and schema are read once, when it is loaded.
pub struct WasmPlugin {
    engine: Engine,
    pre: InstancePre<State>,
    exports: Exports,
    limits: Limits,
    manifest: Manifest,
    schema: Schema,
    /// The fuel the last call used.
    fuel_used: AtomicU64,
}

impl WasmPlugin {
    /// Call one of the plugin's functions in a fresh instance; its text, or why it failed.
    fn text(&self, host: Option<&mut dyn Host>, call: Call<'_>) -> Result<String, String> {
        let (requests, incoming) = mpsc::channel::<Request>();
        let limits = self.limits;
        let outcome = std::thread::scope(|scope| {
            let worker = std::thread::Builder::new()
                .name(format!("plugin {}", self.manifest.id))
                .stack_size(2 * limits.stack_bytes + (4 << 20))
                .spawn_scoped(scope, move || {
                    let state = State { limiter: Limiter { memory_bytes: limits.memory_bytes, refused: None }, requests: Some(requests) };
                    let mut store = Store::new(&self.engine, state);
                    store.limiter(|s| &mut s.limiter);
                    let result =
                        store.set_fuel(limits.fuel).and_then(|()| self.pre.instantiate(&mut store)).and_then(|i| self.call(&mut store, &i, &call));
                    let used = limits.fuel.saturating_sub(store.get_fuel().unwrap_or(0));
                    (result, store.data().limiter.refused, used)
                })
                .map_err(|e| format!("the plugin `{}` could not start ({e})", self.manifest.id))?;
            match host {
                Some(host) => {
                    // Until the worker's store (and with it the sender) is gone.
                    for (command, args, reply) in incoming {
                        let answer = serde_json::from_str::<Value>(&args)
                            .map_err(|e| format!("the arguments of `{command}` are not JSON ({e})"))
                            .and_then(|args| host.run(&command, &args))
                            .map(|v| v.to_string());
                        let _ = reply.send(answer);
                    }
                }
                None => drop(incoming),
            }
            worker.join().map_err(|_| format!("the plugin `{}` failed (the host's worker panicked)", self.manifest.id))
        })?;
        self.fuel_used.store(outcome.2, Ordering::Relaxed);
        match outcome {
            (Ok(r), ..) => r,
            (Err(e), refused, _) => Err(self.failure(&e, refused)),
        }
    }

    fn call(&self, store: &mut Store<State>, instance: &Instance, call: &Call<'_>) -> wasmtime::Result<Result<String, String>> {
        Ok(match *call {
            Call::Manifest | Call::Schema => {
                let index = if matches!(call, Call::Manifest) { &self.exports.manifest } else { &self.exports.schema };
                let (text,) = instance.get_typed_func::<(), (String,)>(&mut *store, index)?.call(&mut *store, ())?;
                Ok(text)
            }
            Call::Evaluate(params, ctx) | Call::Describe(params, ctx) => {
                let index = if matches!(call, Call::Evaluate(..)) { &self.exports.evaluate } else { &self.exports.describe };
                instance.get_typed_func::<(&str, &str), (Result<String, String>,)>(&mut *store, index)?.call(&mut *store, (params, ctx))?.0
            }
        })
    }

    /// Why a call stopped, for the feature's error.
    fn failure(&self, e: &wasmtime::Error, memory_refused: Option<usize>) -> String {
        let id = &self.manifest.id;
        let mib = |b: usize| b.div_ceil(1 << 20);
        if let Some(bytes) = memory_refused {
            return format!(
                "the plugin `{id}` ran out of memory: it asked for {} MiB, the limit is {} MiB",
                mib(bytes),
                mib(self.limits.memory_bytes)
            );
        }
        match e.downcast_ref::<Trap>() {
            Some(Trap::OutOfFuel) => format!(
                "the plugin `{id}` used its whole allowance of {} million instructions and was stopped (it may be stuck in a loop)",
                self.limits.fuel / 1_000_000
            ),
            Some(Trap::StackOverflow) => format!("the plugin `{id}` ran out of stack (the limit is {} KiB)", self.limits.stack_bytes >> 10),
            Some(Trap::UnreachableCodeReached) => format!("the plugin `{id}` stopped (it panicked)"),
            Some(trap) => format!("the plugin `{id}` stopped: {trap}"),
            None => format!("the plugin `{id}` failed: {}", first_line(e)),
        }
    }

    /// The limits each of its calls gets.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The fuel (about the instructions) its last call used: how close a plugin is to its
    /// [`Limits::fuel`].
    pub fn fuel_used(&self) -> u64 {
        self.fuel_used.load(Ordering::Relaxed)
    }
}

impl Plugin for WasmPlugin {
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }
    fn schema(&self) -> Schema {
        self.schema.clone()
    }
    fn evaluate(&self, params: &Value, ctx: &Value, host: &mut dyn Host) -> Result<Value, String> {
        let text = self.text(Some(host), Call::Evaluate(&params.to_string(), &ctx.to_string()))?;
        serde_json::from_str(&text).map_err(|e| format!("the plugin `{}` gave a result that is not JSON ({e})", self.manifest.id))
    }
    fn describe(&self, params: &Value, ctx: &Value) -> Result<Value, String> {
        let text = self.text(None, Call::Describe(&params.to_string(), &ctx.to_string()))?;
        serde_json::from_str(&text).map_err(|e| format!("the plugin `{}` gave a description that is not JSON ({e})", self.manifest.id))
    }
}

fn first_line(e: &wasmtime::Error) -> String {
    e.to_string().lines().next().unwrap_or_default().to_string()
}
