/* xformtest: compare X Render composites (identity and transformed, nearest
 * and bilinear, Over/Add/Src, with and without an a8 mask) against
 * in-process pixman, and time them.
 * Usage: DISPLAY=:0 xformtest [--no-timing] [--dump FILE] [--case N] [--droptest]
 *   --droptest  N back-to-back Add composites of 0x01 onto transparent;
 *               every channel must read min(N, 255). Catches dropped commands.
 * Build: gcc -O2 -Wall -o xformtest xformtest.c \
 *            $(pkg-config --cflags --libs x11 xrender pixman-1) -lm */
#define _GNU_SOURCE
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/extensions/Xrender.h>
#include <pixman.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define DW 256
#define DH 256

static Display *dpy;
static Window root;
static XRenderPictFormat *fmt32, *fmt8;

typedef struct { int w, h, bpp; uint32_t *argb; uint8_t *a8; Pixmap pm; Picture pic; const char *name; int repeat; } Img;

static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec + t.tv_nsec * 1e-9; }

static uint32_t premul(int a, int r, int g, int b) {
    r = (r * a + 127) / 255; g = (g * a + 127) / 255; b = (b * a + 127) / 255;
    return (uint32_t)a << 24 | r << 16 | g << 8 | b;
}
static int clampi(int v) { return v < 0 ? 0 : v > 255 ? 255 : v; }

/* photo-like: smooth colour fields + mid-frequency detail + a few hard edges, opaque to the edge */
static void photo(uint32_t *p, int w, int h, int alpha_grad) {
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) {
            double u = (double)x / w, v = (double)y / h;
            int r = clampi(128 + 100 * sin(6.0 * u + 2.0 * v) + 25 * sin(x * 0.9));
            int g = clampi(128 + 100 * cos(5.0 * v - 3.0 * u) + 25 * cos(y * 0.7));
            int b = clampi(255 * u * v + ((x / 8 + y / 8) & 1 ? 40 : 0));
            if ((x - w / 3) * (x - w / 3) + (y - h / 2) * (y - h / 2) < (w * h) / 40) { r = 250; g = 240; b = 20; }
            int a = alpha_grad ? clampi((int)(255 * (0.15 + 0.85 * u)) - (int)(60 * v)) : 255;
            p[y * w + x] = premul(a, r, g, b);
        }
}

static void put32(Pixmap pm, const uint32_t *data, int w, int h) {
    XImage *im = XCreateImage(dpy, DefaultVisual(dpy, DefaultScreen(dpy)), 32, ZPixmap, 0, (char *)data, w, h, 32, w * 4);
    GC gc = XCreateGC(dpy, pm, 0, NULL);
    XPutImage(dpy, pm, gc, im, 0, 0, 0, 0, w, h);
    XFreeGC(dpy, gc); im->data = NULL; XDestroyImage(im);
}
static void put8(Pixmap pm, const uint8_t *data, int w, int h) {
    int stride = (w + 3) & ~3;
    char *buf = calloc(stride, h);
    for (int y = 0; y < h; y++) memcpy(buf + y * stride, data + y * w, w);
    XImage *im = XCreateImage(dpy, DefaultVisual(dpy, DefaultScreen(dpy)), 8, ZPixmap, 0, buf, w, h, 8, stride);
    GC gc = XCreateGC(dpy, pm, 0, NULL);
    XPutImage(dpy, pm, gc, im, 0, 0, 0, 0, w, h);
    XFreeGC(dpy, gc); XDestroyImage(im);
}

static Img mk32(const char *name, int w, int h, int alpha, int repeat) {
    Img i = { w, h, 32, malloc(w * h * 4), NULL, 0, 0, name, repeat };
    photo(i.argb, w, h, alpha);
    i.pm = XCreatePixmap(dpy, root, w, h, 32);
    put32(i.pm, i.argb, w, h);
    XRenderPictureAttributes pa = { .repeat = repeat ? RepeatNormal : RepeatNone };
    i.pic = XRenderCreatePicture(dpy, i.pm, fmt32, CPRepeat, &pa);
    return i;
}
static Img mk8(const char *name, int w, int h) {
    Img i = { w, h, 8, NULL, malloc(w * h), 0, 0, name, 0 };
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) {
            double dx = x - w / 2.0 + 0.5, dy = y - h / 2.0 + 0.5;
            i.a8[y * w + x] = clampi((int)(255 - 255 * sqrt(dx * dx + dy * dy) / (w * 0.6)) + ((x / 4) & 1 ? 0 : -30));
        }
    i.pm = XCreatePixmap(dpy, root, w, h, 8);
    put8(i.pm, i.a8, w, h);
    i.pic = XRenderCreatePicture(dpy, i.pm, fmt8, 0, NULL);
    return i;
}

typedef struct { const char *name; double a, b, c, d; int centred; int band; } Xf;
/* a,b,c,d = dest->source linear part; centred maps dest centre to source centre */
static Xf xfs[] = {
    { "identity", 1, 0, 0, 1, 0, 1 },
    { "scale0.5", 2, 0, 0, 2, 1, 1 },
    { "scale2", 0.5, 0, 0, 0.5, 1, 2 },
    { "scale1/3.2", 3.2, 0, 0, 3.2, 1, 1 },
    { "rot17", 0, 0, 0, 0, 1, 1 },
    { "rot90", 0, 1, -1, 0, 1, 1 },
    { "skew0.3", 1, -0.3, 0, 1, 1, 1 },
};

static void build(const Xf *x, int sw, int sh, double m[3][3]) {
    double a = x->a, b = x->b, c = x->c, d = x->d;
    if (!strcmp(x->name, "rot17")) { double t = -17 * M_PI / 180; a = cos(t); b = -sin(t); c = sin(t); d = cos(t); }
    double tx = 0, ty = 0;
    if (x->centred) { /* src = A (dst - cd) + cs */
        double cx = DW / 2.0, cy = DH / 2.0;
        tx = sw / 2.0 - (a * cx + b * cy); ty = sh / 2.0 - (c * cx + d * cy);
    }
    m[0][0] = a; m[0][1] = b; m[0][2] = tx; m[1][0] = c; m[1][1] = d; m[1][2] = ty; m[2][0] = 0; m[2][1] = 0; m[2][2] = 1;
}

static void tofixed(double m[3][3], XTransform *xt, pixman_transform_t *pt) {
    for (int i = 0; i < 3; i++)
        for (int j = 0; j < 3; j++) {
            XFixed f = XDoubleToFixed(m[i][j]);
            xt->matrix[i][j] = f; pt->matrix[i][j] = f;
        }
}

static void setpic(Picture p, XTransform *xt, const char *filter) {
    XRenderSetPictureTransform(dpy, p, xt);
    XRenderSetPictureFilter(dpy, p, filter, NULL, 0);
}

static uint32_t *destpat;
static uint32_t *readback(Pixmap pm, int w, int h) {
    XImage *im = XGetImage(dpy, pm, 0, 0, w, h, AllPlanes, ZPixmap);
    uint32_t *o = malloc(w * h * 4);
    for (int y = 0; y < h; y++) memcpy(o + y * w, im->data + y * im->bytes_per_line, w * 4);
    XDestroyImage(im);
    return o;
}

/* footprint membership of dest pixel centre for an image under transform */
static int infp(double m[3][3], int w, int h, int x, int y) {
    double u = m[0][0] * (x + 0.5) + m[0][1] * (y + 0.5) + m[0][2];
    double v = m[1][0] * (x + 0.5) + m[1][1] * (y + 0.5) + m[1][2];
    return u >= 0 && u < w && v >= 0 && v < h;
}
static void markedges(uint8_t *edge, double m[3][3], int w, int h, int band) {
    static uint8_t fp[DW * DH];
    for (int y = 0; y < DH; y++) for (int x = 0; x < DW; x++) fp[y * DW + x] = infp(m, w, h, x, y);
    for (int y = 0; y < DH; y++)
        for (int x = 0; x < DW; x++) {
            for (int dy = -band; dy <= band; dy++)
                for (int dx = -band; dx <= band; dx++) {
                    int xx = x + dx, yy = y + dy;
                    if (xx < 0 || yy < 0 || xx >= DW || yy >= DH) continue;
                    if (fp[yy * DW + xx] != fp[y * DW + x]) { edge[y * DW + x] = 1; goto next; }
                }
        next:;
        }
}

static Pixmap dpm, dpat; static Picture dpic; static GC copygc;
static void prefill(void) { XCopyArea(dpy, dpat, dpm, copygc, 0, 0, DW, DH, 0, 0); }
static FILE *dumpf;
static int ncases, nbad, only_case;

static void run_case(Img *s, const Xf *xf, const char *filter, int op, Img *mk, const char *opname) {
    double m[3][3]; XTransform xt; pixman_transform_t pt;
    if (only_case && ncases + 1 != only_case) { ncases++; return; }
    build(xf, s->w, s->h, m); tofixed(m, &xt, &pt);
    double mm[3][3]; XTransform mxt; pixman_transform_t mpt;
    if (mk) { build(xf, mk->w, mk->h, mm); tofixed(mm, &mxt, &mpt); }
    int bil = !strcmp(filter, "bilinear");

    prefill();
    setpic(s->pic, &xt, filter);
    if (mk) setpic(mk->pic, &mxt, filter);
    XRenderComposite(dpy, op, s->pic, mk ? mk->pic : None, dpic, 0, 0, 0, 0, 0, 0, DW, DH);
    uint32_t *xr = readback(dpm, DW, DH);

    uint32_t *ref = malloc(DW * DH * 4); memcpy(ref, destpat, DW * DH * 4);
    pixman_image_t *pd = pixman_image_create_bits(PIXMAN_a8r8g8b8, DW, DH, ref, DW * 4);
    pixman_image_t *ps = pixman_image_create_bits(PIXMAN_a8r8g8b8, s->w, s->h, s->argb, s->w * 4);
    pixman_image_set_transform(ps, &pt);
    pixman_image_set_filter(ps, bil ? PIXMAN_FILTER_BILINEAR : PIXMAN_FILTER_NEAREST, NULL, 0);
    pixman_image_set_repeat(ps, s->repeat ? PIXMAN_REPEAT_NORMAL : PIXMAN_REPEAT_NONE);
    pixman_image_t *pm = NULL;
    if (mk) {
        pm = pixman_image_create_bits(PIXMAN_a8, mk->w, mk->h, NULL, 0);
        uint8_t *mb = (uint8_t *)pixman_image_get_data(pm); int ms = pixman_image_get_stride(pm);
        for (int y = 0; y < mk->h; y++) memcpy(mb + y * ms, mk->a8 + y * mk->w, mk->w);
        pixman_image_set_transform(pm, &mpt);
        pixman_image_set_filter(pm, bil ? PIXMAN_FILTER_BILINEAR : PIXMAN_FILTER_NEAREST, NULL, 0);
    }
    pixman_image_composite32(op == PictOpOver ? PIXMAN_OP_OVER : op == PictOpAdd ? PIXMAN_OP_ADD : PIXMAN_OP_SRC,
                             ps, pm, pd, 0, 0, 0, 0, 0, 0, DW, DH);

    static uint8_t edge[DW * DH]; memset(edge, 0, sizeof edge);
    int is_ident = !strcmp(xf->name, "identity");
    if (bil && !is_ident && !s->repeat) {
        markedges(edge, m, s->w, s->h, xf->band);
        if (mk) markedges(edge, mm, mk->w, mk->h, xf->band);
    }
    int maxd = 0, nd = 0, ne = 0, ned = 0, maxe = 0;
    uint32_t hash = 2166136261u;
    for (int i = 0; i < DW * DH; i++) {
        int d = 0;
        for (int sh = 0; sh < 32; sh += 8) { int t = abs((int)((xr[i] >> sh) & 255) - (int)((ref[i] >> sh) & 255)); if (t > d) d = t; }
        for (int k = 0; k < 4; k++) hash = (hash ^ ((xr[i] >> (8 * k)) & 255)) * 16777619u;
        if (edge[i]) { ne++; if (d > 2) ned++; if (d > maxe) maxe = d; }
        else { if (d > maxd) maxd = d; if (d > 2) nd++; }
    }
    char mname[32]; snprintf(mname, sizeof mname, "%s", mk ? "a8mask" : "-");
    ncases++; if (nd) nbad++;
    printf("%-3d %-14s %-10s %-8s %-4s %-6s | max %3d  >2 %5d | edge %5d  >2 %5d  max %3d | %08x%s\n",
           ncases, s->name, xf->name, filter, opname, mname, maxd, nd, ne, ned, maxe, hash, nd ? "  MISMATCH" : "");
    if (dumpf) { fwrite(xr, 4, DW * DH, dumpf); }
    if (only_case) { /* bounding boxes of pixels changed from the prefill, X vs pixman */
        int bx[2][4] = { { DW, DH, -1, -1 }, { DW, DH, -1, -1 } };
        for (int y = 0; y < DH; y++) for (int x = 0; x < DW; x++) {
            uint32_t v[2] = { xr[y * DW + x], ref[y * DW + x] };
            for (int k = 0; k < 2; k++) if (v[k] != destpat[y * DW + x]) {
                if (x < bx[k][0]) bx[k][0] = x; if (y < bx[k][1]) bx[k][1] = y;
                if (x > bx[k][2]) bx[k][2] = x; if (y > bx[k][3]) bx[k][3] = y; }
        }
        printf("changed bbox X      : (%d,%d)-(%d,%d)\nchanged bbox pixman : (%d,%d)-(%d,%d)\n",
               bx[0][0], bx[0][1], bx[0][2], bx[0][3], bx[1][0], bx[1][1], bx[1][2], bx[1][3]);
        for (int y = 0; y < 72; y += 8) { for (int x = 0; x < 80; x += 8) printf(" %08x/%08x", xr[y*DW+x], ref[y*DW+x]); printf("\n"); }
        /* map of differing pixels (>2), 1 char per 4x4 block */
        for (int by = 0; by < DH; by += 4) { for (int bx = 0; bx < DW; bx += 4) { int bad = 0;
            for (int y = by; y < by + 4; y++) for (int x = bx; x < bx + 4; x++) { int d = 0;
                for (int sh = 0; sh < 32; sh += 8) { int t = abs((int)((xr[y*DW+x] >> sh) & 255) - (int)((ref[y*DW+x] >> sh) & 255)); if (t > d) d = t; }
                if (d > 2) bad++; }
            putchar(bad ? (bad > 9 ? '#' : '0' + bad) : '.'); } putchar('\n'); }
    }
    free(xr); pixman_image_unref(pd); pixman_image_unref(ps); if (pm) pixman_image_unref(pm); free(ref);
}

static void timing(Img *s, const char *filter, int op, const char *opname) {
    double m[3][3] = { { 3.2, 0, 0 }, { 0, 3.2, 0 }, { 0, 0, 1 } }; XTransform xt; pixman_transform_t pt; tofixed(m, &xt, &pt);
    prefill();
    setpic(s->pic, &xt, filter);
    XRenderComposite(dpy, op, s->pic, None, dpic, 0, 0, 0, 0, 0, 0, 160, 160); XSync(dpy, 0);
    double t0 = now();
    for (int i = 0; i < 200; i++) XRenderComposite(dpy, op, s->pic, None, dpic, 0, 0, 0, 0, 0, 0, 160, 160);
    XSync(dpy, 0);
    XDestroyImage(XGetImage(dpy, dpm, 159, 159, 1, 1, AllPlanes, ZPixmap)); /* waits for the engine */
    double tx = now() - t0;
    uint32_t *ref = malloc(DW * DH * 4); memcpy(ref, destpat, DW * DH * 4);
    pixman_image_t *pd = pixman_image_create_bits(PIXMAN_a8r8g8b8, DW, DH, ref, DW * 4);
    pixman_image_t *ps = pixman_image_create_bits(PIXMAN_a8r8g8b8, s->w, s->h, s->argb, s->w * 4);
    pixman_image_set_transform(ps, &pt);
    pixman_image_set_filter(ps, strcmp(filter, "bilinear") ? PIXMAN_FILTER_NEAREST : PIXMAN_FILTER_BILINEAR, NULL, 0);
    t0 = now();
    for (int i = 0; i < 200; i++) pixman_image_composite32(op == PictOpOver ? PIXMAN_OP_OVER : PIXMAN_OP_SRC, ps, NULL, pd, 0, 0, 0, 0, 0, 0, 160, 160);
    double tp = now() - t0;
    printf("timing %s 512->160 %-8s %-4s: X %8.1f ms total, %6.2f ms/op | in-process pixman %8.1f ms total, %6.2f ms/op\n",
           s->name, filter, opname, tx * 1e3, tx * 5, tp * 1e3, tp * 5);
    pixman_image_unref(pd); pixman_image_unref(ps); free(ref);
}

int main(int argc, char **argv) {
    int do_timing = 1;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--no-timing")) do_timing = 0;
        else if (!strcmp(argv[i], "--dump") && i + 1 < argc) dumpf = fopen(argv[++i], "wb");
        else if (!strcmp(argv[i], "--case") && i + 1 < argc) { only_case = atoi(argv[++i]); do_timing = 0; }
    }
    int droptest = 0;
    for (int i = 1; i < argc; i++) if (!strcmp(argv[i], "--droptest")) droptest = 1;
    dpy = XOpenDisplay(NULL);
    if (!dpy) { fprintf(stderr, "cannot open display\n"); return 1; }
    root = DefaultRootWindow(dpy);
    fmt32 = XRenderFindStandardFormat(dpy, PictStandardARGB32);
    fmt8 = XRenderFindStandardFormat(dpy, PictStandardA8);
    printf("pixman %s\n", pixman_version_string());

    destpat = malloc(DW * DH * 4);
    for (int y = 0; y < DH; y++)
        for (int x = 0; x < DW; x++) {
            int a = ((x / 16 + y / 16) & 1) ? 255 : 160;
            destpat[y * DW + x] = premul(a, (x * 7) & 255, (y * 5) & 255, ((x + y) * 3) & 255);
        }
    dpm = XCreatePixmap(dpy, root, DW, DH, 32);
    dpic = XRenderCreatePicture(dpy, dpm, fmt32, 0, NULL);
    dpat = XCreatePixmap(dpy, root, DW, DH, 32);
    put32(dpat, destpat, DW, DH);
    copygc = XCreateGC(dpy, dpm, 0, NULL);

    if (droptest) { /* N x Add of 0x01010101 onto transparent: expect min(N,255) per channel */
        uint32_t *one = malloc(DW * DH * 4);
        for (int i = 0; i < DW * DH; i++) one[i] = 0x01010101;
        Pixmap opm = XCreatePixmap(dpy, root, DW, DH, 32); put32(opm, one, DW, DH);
        Picture op = XRenderCreatePicture(dpy, opm, fmt32, 0, NULL);
        int ns[] = { 1, 10, 50, 100, 200, 250 };
        for (unsigned k = 0; k < sizeof ns / sizeof ns[0]; k++) {
            XRenderColor z = { 0, 0, 0, 0 };
            XRenderFillRectangle(dpy, PictOpSrc, dpic, &z, 0, 0, DW, DH);
            XSync(dpy, 0);
            double t0 = now();
            for (int i = 0; i < ns[k]; i++) XRenderComposite(dpy, PictOpAdd, op, None, dpic, 0, 0, 0, 0, 0, 0, DW, DH);
            XSync(dpy, 0);
            uint32_t *r = readback(dpm, DW, DH);
            double t = now() - t0;
            int mn = 255, mx = 0;
            for (int i = 0; i < DW * DH; i++) { int v = r[i] & 255; if (v < mn) mn = v; if (v > mx) mx = v; }
            printf("droptest N=%3d: blue min %3d max %3d (expect %3d) center %08x  %.1f ms\n", ns[k], mn, mx, ns[k] > 255 ? 255 : ns[k], r[128 * DW + 128], t * 1e3);
            free(r);
        }
        return 0;
    }
    Img srcs[] = {
        mk32("64-opaque", 64, 64, 0, 0), mk32("64-alpha", 64, 64, 1, 0),
        mk32("200x150-opaque", 200, 150, 0, 0), mk32("200x150-alpha", 200, 150, 1, 0),
        mk32("512-opaque", 512, 512, 0, 0), mk32("512-alpha", 512, 512, 1, 0),
        mk32("64-repeat", 64, 64, 0, 1),
    };
    Img mask = mk8("a8-64", 64, 64);
    const char *filters[] = { "nearest", "bilinear" };
    int ops[] = { PictOpOver, PictOpAdd, PictOpSrc }; const char *opn[] = { "Over", "Add", "Src" };
    int nx = sizeof xfs / sizeof xfs[0];

    printf("#   source         transform  filter   op   mask   | non-edge: max diff, px >2 | edge band: px, px >2, max | X hash\n");
    for (unsigned si = 0; si < sizeof srcs / sizeof srcs[0]; si++)
        for (int xi = 0; xi < nx; xi++)
            for (int fi = 0; fi < 2; fi++)
                for (int oi = 0; oi < 3; oi++)
                    run_case(&srcs[si], &xfs[xi], filters[fi], ops[oi], NULL, opn[oi]);
    int mx[] = { 2, 4, 6 };
    for (int si = 2; si <= 3; si++)
        for (int k = 0; k < 3; k++)
            for (int fi = 0; fi < 2; fi++)
                for (int oi = 0; oi < 2; oi++)
                    run_case(&srcs[si], &xfs[mx[k]], filters[fi], ops[oi], &mask, opn[oi]);
    printf("cases %d, cases with non-edge mismatches (>2): %d\n", ncases, nbad);
    if (do_timing) {
        timing(&srcs[4], "bilinear", PictOpOver, "Over");
        timing(&srcs[4], "nearest", PictOpOver, "Over");
        timing(&srcs[4], "bilinear", PictOpSrc, "Src");
        { /* untransformed 256x256 Over, 200 and 2000 iterations, engine waited by a 1-pixel readback */
            XTransform id = { { { XDoubleToFixed(1), 0, 0 }, { 0, XDoubleToFixed(1), 0 }, { 0, 0, XDoubleToFixed(1) } } };
            setpic(srcs[5].pic, &id, "nearest");
            for (int n = 200; n <= 2000; n *= 10) {
                prefill(); XSync(dpy, 0);
                double t0 = now();
                for (int i = 0; i < n; i++) XRenderComposite(dpy, PictOpOver, srcs[5].pic, None, dpic, 0, 0, 0, 0, 0, 0, 256, 256);
                XSync(dpy, 0);
                XDestroyImage(XGetImage(dpy, dpm, 255, 255, 1, 1, AllPlanes, ZPixmap));
                double t = now() - t0;
                printf("timing 512-alpha identity 256x256 Over x%d: X %8.1f ms total, %6.3f ms/op (%.0f Mpx/s)\n", n, t * 1e3, t * 1e3 / n, 65536.0 * n / t / 1e6);
            }
            uint32_t *ref = malloc(DW * DH * 4); memcpy(ref, destpat, DW * DH * 4);
            pixman_image_t *pd = pixman_image_create_bits(PIXMAN_a8r8g8b8, DW, DH, ref, DW * 4);
            pixman_image_t *ps = pixman_image_create_bits(PIXMAN_a8r8g8b8, 512, 512, srcs[5].argb, 512 * 4);
            double t0 = now();
            for (int i = 0; i < 200; i++) pixman_image_composite32(PIXMAN_OP_OVER, ps, NULL, pd, 0, 0, 0, 0, 0, 0, DW, DH);
            double t = now() - t0;
            printf("timing 512-alpha identity 256x256 Over x200: in-process pixman %8.1f ms total, %6.3f ms/op (%.0f Mpx/s)\n", t * 1e3, t * 1e3 / 200, 65536.0 * 200 / t / 1e6);
        }
    }
    if (dumpf) fclose(dumpf);
    XCloseDisplay(dpy);
    return 0;
}
