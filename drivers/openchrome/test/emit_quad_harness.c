#include <stdio.h>
#include <string.h>
#include <math.h>
#include "via_3d.h"

/*
 * Drives via_3d.c without hardware and prints the command stream it
 * builds, one dword per line (with the float value for vertex data).
 * Usage: harness numTex [transformed bilinear angle scale clipW clipH]
 * Build it against an unpatched tree and a patched one (-DNEW):
 * untransformed output must be identical. See run_harness.sh and
 * check_clip.py.
 */

void ErrorF(const char *f, ...) { (void)f; }

static CARD32 out[4096]; static unsigned nout;
static void flush(ViaCommandBuffer *cb) { memcpy(out + nout, cb->buf, cb->pos * 4); nout += cb->pos; cb->pos = 0; }

static ViaCommandBuffer *newcb(void) {
    static ViaCommandBuffer c; static CARD32 b[16384];
    memset(&c, 0, sizeof c); c.buf = b; c.bufSize = 16384; c.flushFunc = flush; return &c;
}

static int CLIPW, CLIPH;
int run(int numTex, int transformed, int bilinear, double ang, double sc) {
    Via3DState v3d; ViaCommandBuffer *cb = newcb(); int i;
    memset(&v3d, 0, sizeof v3d);
    viaInit3DState(&v3d);
    nout = 0;
    v3d.setDestination(&v3d, 0x100000, 4096, PICT_a8r8g8b8);
    v3d.setFlags(&v3d, numTex, FALSE, TRUE, TRUE);
    for (i = 0; i < numTex; i++)
        v3d.setTexture(&v3d, i, 0x200000 + i * 0x100000, 2048, TRUE, 512, 256, PICT_a8r8g8b8,
                       via_repeat, via_repeat, i ? via_mask : via_src, FALSE);
#ifdef NEW
    struct pixman_transform m; memset(&m, 0, sizeof m);
    m.matrix[0][0] = pixman_double_to_fixed(cos(ang) * sc); m.matrix[0][1] = pixman_double_to_fixed(-sin(ang) * sc);
    m.matrix[1][0] = pixman_double_to_fixed(sin(ang) * sc); m.matrix[1][1] = pixman_double_to_fixed(cos(ang) * sc);
    m.matrix[0][2] = pixman_double_to_fixed(10); m.matrix[2][2] = pixman_fixed_1;
    if (transformed) for (i = 0; i < numTex; i++) v3d.setTexTransform(&v3d, i, &m, bilinear, CLIPW, CLIPH);
#endif
    v3d.emitState(&v3d, cb, TRUE);
    v3d.emitQuad(&v3d, cb, 5, 7, 16, 32, 3, 4, 100, 50);
    for (i = 0; i < (int)nout; i++) {
        float f; memcpy(&f, &out[i], 4);
        printf("%08x %s%g\n", out[i], (fabsf(f) > 1e-4 && fabsf(f) < 1e5) ? "f=" : "", (fabsf(f) > 1e-4 && fabsf(f) < 1e5) ? f : 0.0);
    }
    return 0;
}
int main(int argc, char **argv) {
    int n = atoi(argv[1]), tr = argc > 2 ? atoi(argv[2]) : 0, bl = argc > 3 ? atoi(argv[3]) : 0;
    double ang = argc > 4 ? atof(argv[4]) : 0, sc = argc > 5 ? atof(argv[5]) : 1;
    CLIPW = argc > 6 ? atoi(argv[6]) : 0; CLIPH = argc > 7 ? atoi(argv[7]) : 0;
    return run(n, tr, bl, ang, sc);
}
