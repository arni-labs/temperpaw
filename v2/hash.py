# hash.py <spec.qnt> <out.qnt> <modules dir>: each "@<name>.rs" source_hash becomes that file's sha256, as the bundle needs.
import hashlib, os, re, sys
spec, out, mods = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(spec).read()
def h(m):
    return hashlib.sha256(open(os.path.join(mods, m.group(1)), "rb").read()).hexdigest()
s, n = re.subn(r'@([a-z_]+\.rs)', lambda m: h(m), s)
assert n > 0, "no @module references"
open(out, "w").write(s)
