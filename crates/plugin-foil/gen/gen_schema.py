#!/usr/bin/env python3
"""Generate crates/plugin-foil/src/schema_gen.rs from the Onshape FeatureScript preconditions.

    python3 crates/plugin-foil/gen/gen_schema.py <foil_design_bN.fs> <bseries_blade_bN.fs> > crates/plugin-foil/src/schema_gen.rs && cargo fmt -p solidcraft-plugin-foil

Each feature's precondition becomes a list of fields in dialog order: key, label, group, the
FeatureScript's bounds and default, its description (the tooltip) and the condition under which
Onshape shows it (nested `if`s joined, `else` as the negation). Enum fields carry their options.
Nothing is hand-edited in the output; re-run this when the FeatureScript changes.
"""
import re
import sys

FEATURES = [
    # (file index, Feature Type Name, Rust const)
    (0, "Foil wing", "WING"),
    (0, "Foil stabiliser", "STAB"),
    (1, "B-Series Propeller", "PROP"),
]


def enums(src):
    out = {}
    for m in re.finditer(r"export enum (\w+)\s*\{(.*?)\n\}", src, re.S):
        vals = re.findall(r'annotation \{ "Name" : "((?:[^"\\]|\\.)*)" \}\s*(\w+)', m.group(2))
        out[m.group(1)] = [(v, n) for n, v in vals]
    return out


def balanced(src, i, open_c, close_c):
    """Index just past the bracket matching src[i] (== open_c), skipping strings."""
    depth = 0
    j = i
    while j < len(src):
        c = src[j]
        if c == '"':
            j += 1
            while src[j] != '"':
                j += 2 if src[j] == "\\" else 1
        elif c == open_c:
            depth += 1
        elif c == close_c:
            depth -= 1
            if depth == 0:
                return j + 1
        j += 1
    raise ValueError("unbalanced")


def precondition(src, name):
    i = src.index('"Feature Type Name" : "%s"' % name)
    i = src.index("precondition", i)
    i = src.index("{", i)
    return src[i:balanced(src, i, "{", "}")]


def strings(annot):
    d = {}
    for k, v in re.findall(r'"([\w ]+)"\s*:\s*("(?:[^"\\]|\\.)*"|true|false|\w+)', annot):
        d[k] = v[1:-1] if v.startswith('"') else v
    return d


def cond_text(c):
    c = re.sub(r"definition\.", "", c)
    c = re.sub(r"\b[A-Z]\w*\.([A-Z0-9_]+)\b", r"\1", c)
    return " ".join(c.split())


def walk(body, ens, out, conds, group):
    """Fields of a precondition block, in order."""
    i = 0
    pending = None
    last_if = None
    group_next = None
    while i < len(body):
        if body[i].isspace():
            i += 1
            continue
        if body.startswith("//", i):
            i = body.index("\n", i)
            continue
        if body.startswith("annotation", i):
            j = body.index("{", i)
            k = balanced(body, j, "{", "}")
            a = strings(body[j:k])
            if "Group Name" in a:
                group_next = (a["Group Name"], a.get("Collapsed By Default", "false") == "true")
            else:
                pending = a
            i = k
            continue
        if body.startswith("if", i) and re.match(r"if\s*\(", body[i:]):
            j = body.index("(", i)
            k = balanced(body, j, "(", ")")
            c = cond_text(body[j + 1:k - 1])
            j = body.index("{", k)
            e = balanced(body, j, "{", "}")
            walk(body[j + 1:e - 1], ens, out, conds + [c], group)
            last_if = c
            i = e
            continue
        if body.startswith("else", i):
            j = body.index("{", i)
            e = balanced(body, j, "{", "}")
            walk(body[j + 1:e - 1], ens, out, conds + ["!(%s)" % last_if], group)
            i = e
            continue
        if body[i] == "{":
            e = balanced(body, i, "{", "}")
            g = group_next if group_next else group
            group_next = None
            walk(body[i + 1:e - 1], ens, out, conds, g)
            i = e
            continue
        j = body.index(";", i)
        stmt = " ".join(body[i:j].split())
        i = j + 1
        m = re.match(r"is(Real|Length|Angle|Integer)\(definition\.(\w+), \{ \((\w+)\) : \[([^\]]*)\] \}", stmt)
        kind = None
        if m:
            lo, de, hi = [float(x) for x in m.group(4).split(",")]
            kind = (m.group(1), lo, de, hi)
            key = m.group(2)
        else:
            m = re.match(r"definition\.(\w+) is (\w+)", stmt)
            if not m:
                raise ValueError("unknown statement: " + stmt)
            key, ty = m.group(1), m.group(2)
            if ty == "boolean":
                kind = ("Bool", (pending or {}).get("Default", "false") == "true")
            else:
                opts = ens[ty]
                d = (pending or {}).get("Default", opts[0][0])
                kind = ("Enum", ty, d)
        a = pending or {}
        pending = None
        out.append(dict(key=key, label=a.get("Name", key), desc=a.get("Description", ""), kind=kind,
                        cond=" && ".join("(%s)" % c if ("||" in c or "&&" in c) and len(conds) > 1 else c for c in conds),
                        group=group))


def rs(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def num(x):
    s = repr(float(x))
    return s


def main():
    srcs = [open(p).read() for p in sys.argv[1:3]]
    ens = [enums(s) for s in srcs]
    print("// Generated by crates/plugin-foil/gen/gen_schema.py from the Onshape FeatureScript: do not edit.")
    print("// Foil design tool build %s, B-Series propeller build %s." % (
        re.search(r'FOIL_FILE_BUILD = "(\d+)"', srcs[0]).group(1), re.search(r'PROP_FILE_BUILD = "(\d+)"', srcs[1]).group(1)))
    print("#![allow(clippy::all)]\n")
    print("use crate::schema::{Field, Kind};\n")
    used = {}
    for fi, name, const in FEATURES:
        out = []
        walk(precondition(srcs[fi], name)[1:-1], ens[fi], out, [], ("", False))
        for f in out:
            if f["kind"][0] == "Enum":
                used[(fi, f["kind"][1])] = True
        print("pub static %s: &[Field] = &[" % const)
        for f in out:
            k = f["kind"]
            if k[0] == "Bool":
                kk = "Kind::Bool { default: %s }" % ("true" if k[1] else "false")
            elif k[0] == "Enum":
                kk = "Kind::Enum { options: %s_%s, default: %s }" % (["FOIL", "PROP"][fi], k[1].upper(), rs(k[2]))
            else:
                kk = "Kind::%s { min: %s, default: %s, max: %s }" % ({"Integer": "Int"}.get(k[0], k[0]), num(k[1]), num(k[2]), num(k[3]))
            g, collapsed = f["group"]
            print("    Field { key: %s, label: %s, group: %s, collapsed: %s, kind: %s, when: %s, help: %s }," % (
                rs(f["key"]), rs(f["label"]), rs(g), "true" if collapsed else "false", kk, rs(f["cond"]), rs(f["desc"])))
        print("];\n")
    for (fi, ty) in sorted(used):
        opts = ens[fi][ty]
        print("pub static %s_%s: &[(&str, &str)] = &[%s];" % (["FOIL", "PROP"][fi], ty.upper(),
              ", ".join("(%s, %s)" % (rs(v), rs(n)) for v, n in opts)))


main()
