/* solidtest: compare X Render composites that use solid-fill pictures (RenderCreateSolidFill)
 * and RenderFillRectangles against in-process pixman, on 24-bit and 32-bit destinations.
 * These are what openchrome patches 0012/0013 move to the 3D/2D engines.
 * Tolerance: 3 per channel (the 3D blend rounds twice), 14 inside transformed bilinear masks
 * (hardware weight precision, as in xformtest), and 2 px at their edges are skipped.
 * Usage: DISPLAY=:0 solidtest [--timing] */
#define _GNU_SOURCE
#include <X11/Xlib.h>
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
static XRenderPictFormat *fmt32, *fmt24, *fmt8;
static int nbad, ncases;

static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec + t.tv_nsec * 1e-9; }
static int clampi(int v) { return v < 0 ? 0 : v > 255 ? 255 : v; }
static uint32_t premul(int a, int r, int g, int b) {
    return (uint32_t)a << 24 | ((r * a + 127) / 255) << 16 | ((g * a + 127) / 255) << 8 | ((b * a + 127) / 255);
}

static void put(Pixmap pm, int depth, const void *data, int w, int h, int bpp) {
    int stride = bpp == 8 ? (w + 3) & ~3 : w * 4;
    char *buf = calloc(stride, h);
    for (int y = 0; y < h; y++) memcpy(buf + y * stride, (const char *)data + y * w * (bpp / 8), w * (bpp / 8));
    XImage *im = XCreateImage(dpy, DefaultVisual(dpy, DefaultScreen(dpy)), depth, ZPixmap, 0, buf, w, h, bpp, stride);
    GC gc = XCreateGC(dpy, pm, 0, NULL);
    XPutImage(dpy, pm, gc, im, 0, 0, 0, 0, w, h);
    XFreeGC(dpy, gc);
    XDestroyImage(im);
}

typedef struct { int w, h; uint32_t *argb; uint8_t *a8; Picture pic; } Img;

static Img argb_img(int w, int h) {
    Img i = { w, h, malloc(w * h * 4), NULL, 0 };
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) {
            int a = clampi((int)(255 * (1.0 - fabs((double)y / h - 0.5) * 2) * (1.0 - (double)x / w * 0.7)));
            i.argb[y * w + x] = premul(a, 255, 255, 255);
        }
    Pixmap pm = XCreatePixmap(dpy, root, w, h, 32);
    put(pm, 32, i.argb, w, h, 32);
    i.pic = XRenderCreatePicture(dpy, pm, fmt32, 0, NULL);
    return i;
}
static Img photo_img(int w, int h) {
    Img i = { w, h, malloc(w * h * 4), NULL, 0 };
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++)
            i.argb[y * w + x] = premul(255, clampi(128 + 100 * sin(x * 0.11)), clampi(128 + 100 * cos(y * 0.07)), (x ^ y) & 255);
    Pixmap pm = XCreatePixmap(dpy, root, w, h, 32);
    put(pm, 32, i.argb, w, h, 32);
    i.pic = XRenderCreatePicture(dpy, pm, fmt32, 0, NULL);
    return i;
}
static Img a8_img(int w, int h) {
    Img i = { w, h, NULL, malloc(w * h), 0 };
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) {
            double dx = x - w / 2.0 + 0.5, dy = y - h / 2.0 + 0.5;
            i.a8[y * w + x] = clampi((int)(255 - 255 * sqrt(dx * dx + dy * dy) / (w * 0.6)) + ((x / 3) & 1 ? 0 : -40));
        }
    Pixmap pm = XCreatePixmap(dpy, root, w, h, 8);
    put(pm, 8, i.a8, w, h, 8);
    i.pic = XRenderCreatePicture(dpy, pm, fmt8, 0, NULL);
    return i;
}

static pixman_image_t *pix_of(Img *i) {
    if (i->argb) return pixman_image_create_bits(PIXMAN_a8r8g8b8, i->w, i->h, i->argb, i->w * 4);
    pixman_image_t *p = pixman_image_create_bits(PIXMAN_a8, i->w, i->h, NULL, 0);
    uint8_t *b = (uint8_t *)pixman_image_get_data(p);
    int s = pixman_image_get_stride(p);
    for (int y = 0; y < i->h; y++) memcpy(b + y * s, i->a8 + y * i->w, i->w);
    return p;
}

static pixman_op_t pop(int op) {
    switch (op) {
    case PictOpSrc: return PIXMAN_OP_SRC;
    case PictOpOver: return PIXMAN_OP_OVER;
    case PictOpAdd: return PIXMAN_OP_ADD;
    case PictOpOutReverse: return PIXMAN_OP_OUT_REVERSE;
    default: return PIXMAN_OP_OVER;
    }
}
static const char *opname(int op) {
    return op == PictOpSrc ? "Src" : op == PictOpOver ? "Over" : op == PictOpAdd ? "Add" : op == PictOpOutReverse ? "OutRev" : "?";
}

/* destination: a pixmap of the given depth with a pattern */
typedef struct { int depth; Pixmap pm, pat; Picture pic; uint32_t *init; GC gc; } Dst;
static Dst mkdst(int depth) {
    Dst d = { depth, XCreatePixmap(dpy, root, DW, DH, depth), XCreatePixmap(dpy, root, DW, DH, depth), 0, malloc(DW * DH * 4), 0 };
    for (int y = 0; y < DH; y++)
        for (int x = 0; x < DW; x++) {
            int a = depth == 24 ? 255 : clampi(64 + x);
            d.init[y * DW + x] = premul(a, (x * 3) & 255, (y * 5) & 255, ((x + y) * 2) & 255);
        }
    put(d.pat, depth, d.init, DW, DH, 32);
    d.pic = XRenderCreatePicture(dpy, d.pm, depth == 24 ? fmt24 : fmt32, 0, NULL);
    d.gc = XCreateGC(dpy, d.pm, 0, NULL);
    return d;
}
static void prefill(Dst *d) { XCopyArea(dpy, d->pat, d->pm, d->gc, 0, 0, DW, DH, 0, 0); }
static uint32_t *readback(Dst *d) {
    XImage *im = XGetImage(dpy, d->pm, 0, 0, DW, DH, AllPlanes, ZPixmap);
    uint32_t *o = malloc(DW * DH * 4);
    for (int y = 0; y < DH; y++) memcpy(o + y * DW, im->data + y * im->bytes_per_line, DW * 4);
    XDestroyImage(im);
    return o;
}
static pixman_image_t *pixdst(Dst *d, uint32_t **mem) {
    *mem = malloc(DW * DH * 4);
    memcpy(*mem, d->init, DW * DH * 4);
    return pixman_image_create_bits(d->depth == 24 ? PIXMAN_x8r8g8b8 : PIXMAN_a8r8g8b8, DW, DH, *mem, DW * 4);
}

/* For transformed masks: pixels within 2 px of the mask's footprint edge, where the 3D engine's
 * bilinear edge differs from pixman's by up to half a pixel (see patch 0003). */
static uint8_t edge[DW * DH];
static void mark_edges(XTransform *xt, int w, int h) {
    static uint8_t fp[DW * DH];
    memset(edge, 0, sizeof edge);
    for (int y = 0; y < DH; y++)
        for (int x = 0; x < DW; x++) {
            double u = XFixedToDouble(xt->matrix[0][0]) * (x + .5) + XFixedToDouble(xt->matrix[0][1]) * (y + .5) + XFixedToDouble(xt->matrix[0][2]);
            double v = XFixedToDouble(xt->matrix[1][0]) * (x + .5) + XFixedToDouble(xt->matrix[1][1]) * (y + .5) + XFixedToDouble(xt->matrix[1][2]);
            fp[y * DW + x] = u >= 0 && u < w && v >= 0 && v < h;
        }
    for (int y = 0; y < DH; y++)
        for (int x = 0; x < DW; x++)
            for (int dy = -2; dy <= 2; dy++)
                for (int dx = -2; dx <= 2; dx++) {
                    int xx = x + dx, yy = y + dy;
                    if (xx >= 0 && yy >= 0 && xx < DW && yy < DH && fp[yy * DW + xx] != fp[y * DW + x]) edge[y * DW + x] = 1;
                }
}

static void compare(const char *name, Dst *d, uint32_t *xr, uint32_t *ref, int tol) {
    int maxd = 0, nd = 0;
    uint32_t cmpmask = d->depth == 24 ? 0x00ffffff : 0xffffffff;
    for (int i = 0; i < DW * DH; i++) {
        uint32_t a = xr[i] & cmpmask, b = ref[i] & cmpmask;
        int dd = 0;
        for (int s = 0; s < 32; s += 8) { int t = abs((int)((a >> s) & 255) - (int)((b >> s) & 255)); if (t > dd) dd = t; }
        if (edge[i]) continue;
        if (dd > maxd) maxd = dd;
        if (dd > tol) nd++;
    }
    ncases++;
    if (nd) nbad++;
    printf("%-3d d%-2d %-52s | max %3d  >%d %5d%s\n", ncases, d->depth, name, maxd, tol, nd, nd ? "  MISMATCH" : "");
}

static Picture solid(uint32_t argb) { /* straight ARGB in, as XRenderColor (premultiplied 16-bit) */
    int a = argb >> 24;
    XRenderColor c = { (unsigned short)(((argb >> 16) & 255) * a / 255 * 257), (unsigned short)(((argb >> 8) & 255) * a / 255 * 257),
                       (unsigned short)((argb & 255) * a / 255 * 257), (unsigned short)(a * 257) };
    return XRenderCreateSolidFill(dpy, &c);
}
static pixman_image_t *psolid(uint32_t argb) {
    int a = argb >> 24;
    pixman_color_t c = { (uint16_t)(((argb >> 16) & 255) * a / 255 * 257), (uint16_t)(((argb >> 8) & 255) * a / 255 * 257),
                         (uint16_t)((argb & 255) * a / 255 * 257), (uint16_t)(a * 257) };
    return pixman_image_create_solid_fill(&c);
}

static void rot(double ang, int sw, int sh, int dx, int dy, XTransform *xt, pixman_transform_t *pt) {
    /* dest -> source: rotate about the source centre placed at dest (dx, dy) */
    double c = cos(ang), s = sin(ang), m[3][3] = { { c, s, 0 }, { -s, c, 0 }, { 0, 0, 1 } };
    m[0][2] = sw / 2.0 - (c * dx + s * dy);
    m[1][2] = sh / 2.0 - (-s * dx + c * dy);
    for (int i = 0; i < 3; i++)
        for (int j = 0; j < 3; j++) xt->matrix[i][j] = pt->matrix[i][j] = XDoubleToFixed(m[i][j]);
}

/* solid source, optional mask image (optionally rotated bilinear), optional solid mask */
static void case_solid(Dst *d, int op, uint32_t col, Img *mask, double angle, int solidmask_alpha, Img *srcimg) {
    char name[96];
    XTransform xt; pixman_transform_t pt;
    prefill(d);
    Picture s = srcimg ? srcimg->pic : solid(col), m = None;
    pixman_image_t *ps = srcimg ? pix_of(srcimg) : psolid(col), *pm = NULL;
    if (mask) {
        m = mask->pic;
        pm = pix_of(mask);
        if (angle != 0) {
            rot(angle, mask->w, mask->h, DW / 2, DH / 2, &xt, &pt);
            mark_edges(&xt, mask->w, mask->h);
            XRenderSetPictureTransform(dpy, m, &xt);
            XRenderSetPictureFilter(dpy, m, "bilinear", NULL, 0);
            pixman_image_set_transform(pm, &pt);
            pixman_image_set_filter(pm, PIXMAN_FILTER_BILINEAR, NULL, 0);
        }
    } else if (solidmask_alpha >= 0) {
        m = solid((uint32_t)solidmask_alpha << 24);
        pm = psolid((uint32_t)solidmask_alpha << 24);
    }
    int x0 = 20, y0 = 30, w = 200, h = 180;
    XRenderComposite(dpy, op, s, m, d->pic, x0, y0, x0, y0, x0, y0, w, h);
    uint32_t *xr = readback(d), *mem;
    pixman_image_t *pd = pixdst(d, &mem);
    pixman_image_composite32(pop(op), ps, pm, pd, x0, y0, x0, y0, x0, y0, w, h);
    snprintf(name, sizeof name, "%s %s %08x mask %s%s", opname(op), srcimg ? "image" : "solid", srcimg ? 0 : col,
             mask ? (mask->a8 ? "a8" : "argb") : solidmask_alpha >= 0 ? "solid" : "-", angle != 0 ? " rotated bilinear" : "");
    if (solidmask_alpha >= 0 && !mask) snprintf(name + strlen(name), sizeof name - strlen(name), " a%d", solidmask_alpha);
    compare(name, d, xr, mem, angle != 0 ? 14 : 3);
    memset(edge, 0, sizeof edge);
    if (mask && angle != 0) {
        XTransform id = { { { XDoubleToFixed(1), 0, 0 }, { 0, XDoubleToFixed(1), 0 }, { 0, 0, XDoubleToFixed(1) } } };
        XRenderSetPictureTransform(dpy, m, &id);
        XRenderSetPictureFilter(dpy, m, "nearest", NULL, 0);
    }
    if (!srcimg) XRenderFreePicture(dpy, s);
    if (m && !mask) XRenderFreePicture(dpy, m);
    free(xr); free(mem);
    pixman_image_unref(pd); pixman_image_unref(ps);
    if (pm) pixman_image_unref(pm);
}

static void case_fill(Dst *d, int op, uint32_t col) {
    char name[96];
    prefill(d);
    int a = col >> 24;
    XRenderColor c = { (unsigned short)(((col >> 16) & 255) * a / 255 * 257), (unsigned short)(((col >> 8) & 255) * a / 255 * 257),
                       (unsigned short)((col & 255) * a / 255 * 257), (unsigned short)(a * 257) };
    XRectangle r[3] = { { 10, 10, 100, 50 }, { 60, 40, 150, 120 }, { 0, 200, 256, 30 } };
    XRenderFillRectangles(dpy, op, d->pic, &c, r, 3);
    uint32_t *xr = readback(d), *mem;
    pixman_image_t *pd = pixdst(d, &mem);
    pixman_color_t pc = { c.red, c.green, c.blue, c.alpha };
    pixman_box32_t b[3] = { { 10, 10, 110, 60 }, { 60, 40, 210, 160 }, { 0, 200, 256, 230 } };
    pixman_image_fill_boxes(pop(op), pd, &pc, 3, b);
    snprintf(name, sizeof name, "FillRectangles %s %08x", opname(op), col);
    compare(name, d, xr, mem, 3);
    free(xr); free(mem);
    pixman_image_unref(pd);
}

static void timing(Dst *d, Img *beam, Img *a8) {
    XTransform xt; pixman_transform_t pt;
    rot(-0.4, beam->w, beam->h, DW / 2, DH / 2, &xt, &pt);
    XRenderSetPictureTransform(dpy, beam->pic, &xt);
    XRenderSetPictureFilter(dpy, beam->pic, "bilinear", NULL, 0);
    struct { const char *name; Picture s, m; int op; } t[] = {
        { "solid through rotated argb mask, Add (the beam)", solid(0xff20a0ff), beam->pic, PictOpAdd },
        { "solid through a8 mask, Over (a glyph run)", solid(0xffc0c0c0), a8->pic, PictOpOver },
        { "translucent solid, Over (a highlight bar)", solid(0x40ffffff), None, PictOpOver },
    };
    for (unsigned k = 0; k < sizeof t / sizeof t[0]; k++) {
        XRenderComposite(dpy, t[k].op, t[k].s, t[k].m, d->pic, 0, 0, 0, 0, 0, 0, DW, DH);
        XSync(dpy, 0);
        double t0 = now();
        for (int i = 0; i < 100; i++) XRenderComposite(dpy, t[k].op, t[k].s, t[k].m, d->pic, 0, 0, 0, 0, 0, 0, DW, DH);
        XDestroyImage(XGetImage(dpy, d->pm, DW - 1, DH - 1, 1, 1, AllPlanes, ZPixmap));
        printf("timing d%d %-48s 256x256: %6.2f ms/op\n", d->depth, t[k].name, (now() - t0) * 10);
    }
}

int main(int argc, char **argv) {
    int do_timing = argc > 1 && !strcmp(argv[1], "--timing");
    dpy = XOpenDisplay(NULL);
    if (!dpy) { fprintf(stderr, "no display\n"); return 2; }
    root = DefaultRootWindow(dpy);
    fmt32 = XRenderFindStandardFormat(dpy, PictStandardARGB32);
    fmt24 = XRenderFindStandardFormat(dpy, PictStandardRGB24);
    fmt8 = XRenderFindStandardFormat(dpy, PictStandardA8);
    Img beam = argb_img(160, 40), a8 = a8_img(DW, DH), photo = photo_img(DW, DH);
    uint32_t cols[] = { 0xff20a0ff, 0x80ff4000, 0x10ffffff };
    int depths[] = { 24, 32 };
    for (int di = 0; di < 2; di++) {
        Dst d = mkdst(depths[di]);
        for (unsigned ci = 0; ci < 3; ci++) {
            case_solid(&d, PictOpOver, cols[ci], NULL, 0, -1, NULL);
            case_solid(&d, PictOpAdd, cols[ci], NULL, 0, -1, NULL);
            case_solid(&d, PictOpOver, cols[ci], &a8, 0, -1, NULL);
            case_solid(&d, PictOpAdd, cols[ci], &a8, 0, -1, NULL);
            case_solid(&d, PictOpAdd, cols[ci], &beam, -0.4, -1, NULL);
            case_solid(&d, PictOpOver, cols[ci], &beam, 0.3, -1, NULL);
            case_solid(&d, PictOpOver, cols[ci], NULL, 0, 128, NULL);
        }
        case_solid(&d, PictOpOver, 0, NULL, 0, 100, &photo);
        case_solid(&d, PictOpAdd, 0, NULL, 0, 200, &photo);
        case_solid(&d, PictOpOutReverse, 0xff000000, &a8, 0, -1, NULL);
        case_fill(&d, PictOpSrc, 0xff102030);
        case_fill(&d, PictOpOver, 0xff102030);
        case_fill(&d, PictOpOver, 0x3cffffff);
        case_fill(&d, PictOpAdd, 0x40204060);
        if (do_timing) timing(&d, &beam, &a8);
    }
    printf("%d cases, %d mismatched\n", ncases, nbad);
    return nbad != 0;
}
