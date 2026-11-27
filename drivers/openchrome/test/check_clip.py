# Checks the polygon via3DEmitQuad emits for transformed pictures without
# repeat: every vertex must map inside the picture (half a texel in for
# bilinear) and the triangles must cover exactly the destination points
# that do. Run from the directory holding the harness binary 'new'.
import sys, struct, math, random, subprocess
def run(args):
    out = subprocess.run(['./new'] + [str(a) for a in args], capture_output=True, text=True).stdout.split('\n')
    return [int(l.split()[0], 16) for l in out if l.strip()]
def verts(dw, numTex):
    # find the 0xEC sub-address (vertex format) then 0xEE, then vertices until next 0xEE
    k = [k for k, d in enumerate(dw) if d >> 24 == 0xEC]
    if not k: return []
    i = k[0] + 2
    vs = []; stride = 3 + 2 * numTex
    while dw[i] >> 24 != 0xEE:
        f = [struct.unpack('<f', struct.pack('<I', d))[0] for d in dw[i:i+stride]]
        vs.append(f); i += stride
    return vs
def inside_tri(p, a, b, c):
    def cr(o, u, v): return (u[0]-o[0])*(v[1]-o[1]) - (u[1]-o[1])*(v[0]-o[0])
    d1, d2, d3 = cr(a, b, p), cr(b, c, p), cr(c, a, p)
    return not ((d1 < -1e-6 or d2 < -1e-6 or d3 < -1e-6) and (d1 > 1e-6 or d2 > 1e-6 or d3 > 1e-6))
fails = 0
for ang, sc, bil, cw, ch in [(0.5236, 1.3, 1, 300, 200), (0, 2, 1, 100, 60), (0, 2, 1, 300, 120), (1.2, 0.7, 0, 64, 64), (0, 0.25, 1, 400, 250), (2.0, 1.0, 1, 20, 20)]:
    dw = run([1, 1, bil, ang, sc, cw, ch]); vs = verts(dw, 1)
    lo = 0.5 if bil else 0.0
    # Render transform as the harness builds it
    a, b, c = math.cos(ang)*sc, -math.sin(ang)*sc, 10.0
    d, e, f = math.sin(ang)*sc, math.cos(ang)*sc, 0.0
    def src(px, py):  # dst-relative point -> source
        X, Y = 16 + px, 32 + py
        return a*X + b*Y + c, d*X + e*Y + f
    tris = [vs[k:k+3] for k in range(0, len(vs), 3)]
    bad = 0
    for v in vs:
        s, t = v[3]*512, v[4]*256
        if not (lo - 1e-3 <= s <= cw - lo + 1e-3 and lo - 1e-3 <= t <= ch - lo + 1e-3): bad += 1
        es, et = src(v[0]-5, v[1]-7)
        if abs(es - s) > 1e-2 or abs(et - t) > 1e-2: bad += 1
    random.seed(1); mism = 0
    for _ in range(4000):
        px, py = random.uniform(0, 100), random.uniform(0, 50)
        sx, sy = src(px, py)
        want = lo < sx < cw - lo and lo < sy < ch - lo
        have = any(inside_tri((px+5, py+7), t[0], t[1], t[2]) for t in tris)
        if want != have:
            # tolerate points within 1e-3 of an edge
            if min(abs(sx-lo), abs(sx-(cw-lo)), abs(sy-lo), abs(sy-(ch-lo))) > 1e-2: mism += 1
    print(f"angle={ang} scale={sc} bilinear={bil} clip={cw}x{ch}: {len(vs)} vertices, bad vertices={bad}, coverage mismatches={mism}")
    fails += bad + mism
sys.exit(1 if fails else 0)
