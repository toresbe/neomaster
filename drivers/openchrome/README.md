# OpenChrome X driver patches

Patches for Debian's `xserver-xorg-video-openchrome` (upstream 0.6.0), the X
driver the BM5's VIA CX700 uses on bookworm. Stock 0.6.0 crashes at start-up
on the BM5 under bookworm's Xorg 21.1, so patch 0001 is needed to get X
running at all. Neomaster's XRender backend
(`crates/render/src/xrender.rs`) scales cover art and rotates the pointer beam
with Render picture transforms and bilinear filtering. The stock driver gets
those wrong. Background and the wider survey are in
[OpenChrome: unused acceleration on the CX700](https://claude.ai/artifact/KMuJdTNGui9KrX2vXBB76f).

| Patch | What it does | Default |
| --- | --- | --- |
| `0001` | Names the panel's native mode, which the driver generates itself because the BM5's panel has no EDID. Without a name, Xorg 21.1 segfaults in `xf86ProbeOutputModes` | Always on |
| `0002` | EXA composite falls back to software for anything the 3D engine can't draw: picture transforms, convolution filters, RepeatPad/Reflect, repeating pictures that aren't a power of two in size, and masks without a drawable | Always on |
| `0003` | Affine transforms (scale, rotate, skew) with nearest or bilinear filtering on the UniChrome 3D engine | Off; enable with `Option "ExaTransform" "true"` |

**Without 0002,** a transformed source over 400 pixels is drawn as an
unscaled crop of the image. Patches 0001 and 0002 make neomaster render
correctly, with software scaling. Patch 0003 moves the scaling to the GPU.

## Status

0001 runs on the BM5 (2026-11-28, as Debian package `1:0.6.0-5+nm1`). X comes
up in UMS with EXA enabled, and FP-1 runs at 1024x768. 0002 and 0003 haven't
run on hardware yet. Offline checks so far:

- All three patches apply to the 0.6.0 tarball and build against Xorg 21.1
  without new warnings.
- `test/run_harness.sh` drives `via_3d.c` without hardware. Untransformed
  composites produce byte-for-byte the same command stream as before. The
  bilinear filter bits and transformed texture coordinates come out as
  expected. The clipped polygon for pictures without repeat covers exactly
  the right pixels.

Hardware facts the code relies on, all still to be confirmed with rendercheck:

- Filter bits in HTXnTB (manual Part 2 p.77) and the Mesa unichrome driver's use of them.
- GL-style sampling at pixel centres, so no half-texel offset is applied.
- Clamp wrap mode.

## The start-up crash (0001)

The BM5's LVDS panel has no DDC channel, so `via_lvds_get_modes` makes the
native mode with `xf86CVTMode()`. On Debian's Xorg 21.1.7 that mode has no
name. `xf86ProbeOutputModes` then calls `strcmp` on the NULL name while
pruning duplicate modes (the inlined `xf86SortModes`), at
`xf86ProbeOutputModes+0x62e` in bookworm's Xorg binary. The fix calls
`xf86SetModeDefaultName()` on the mode. Upstream openchrome main has the
same code.

## How 0003 works

- `via3DEmitQuad` writes each texture's source position as an affine function
  of the destination position and derives per-vertex texture coordinates from
  it.
- The texture unit can only wrap or clamp, and only at the power-of-two
  texture size. So a transformed picture **without repeat** is handled
  differently: the quad is clipped to the region whose samples all fall
  inside the picture (half a texel in for bilinear) and drawn as a triangle
  fan. Outside that region Render's source is transparent. That is only
  equivalent for operators where a transparent source leaves the destination
  alone (Over, OverReverse, Atop, Xor, Add); other operators fall back.
- Compared with pixman, the outermost half pixel of a bilinear picture is left
  out instead of fading to transparent. That only shows when the image is
  opaque right up to its edge.
- Software fallbacks remain for:
  - projective transforms
  - window (non-pixmap) sources
  - pictures over 2048x2048
  - Chrome9 (H6) chips
- Bilinear without mipmaps aliases when shrinking by more than 2x, exactly
  like pixman's bilinear. Neomaster should decode covers near their display
  size.

## Building on the BM5 (bookworm, i386)

```sh
sudo apt-get install build-essential devscripts quilt
sudo apt-get build-dep xserver-xorg-video-openchrome   # needs a deb-src line
apt-get source xserver-xorg-video-openchrome
cd xserver-xorg-video-openchrome-0.6.0
cp ~/neomaster/drivers/openchrome/patches/0*.patch debian/patches/
cat ~/neomaster/drivers/openchrome/patches/series >> debian/patches/series
dch --local +neomaster "Composite transform fixes from neomaster"
dpkg-buildpackage -b -uc -us
sudo dpkg -i ../xserver-xorg-video-openchrome_*+neomaster*_i386.deb
```

Debian's source already carries the GCC 10 `-fno-common` fix. If you build
the plain upstream 0.6.0 tarball instead, configure it with `CFLAGS=-fcommon`.

To enable 0003, add `/etc/X11/xorg.conf.d/20-openchrome.conf`:

```
Section "Device"
    Identifier "CX700"
    Driver     "openchrome"
    Option     "ExaTransform" "true"
EndSection
```

To roll back, delete that file. To remove the patches as well, run
`sudo apt-get install --reinstall xserver-xorg-video-openchrome`.

## Hardware test plan

1. **Confirm the driver mode.** `grep -E "EXA|DRI|KMS|transformed" /var/log/Xorg.0.log`
   should show "[EXA] Enabled EXA acceleration" and "EXA transformed and
   bilinear composites enabled/disabled". The driver must be in UMS, which it
   is when the out-of-tree openchrome KMS module isn't loaded.
2. **Run rendercheck three times:** stock package, patched with `ExaTransform`
   off, and patched with it on. Install it with `sudo apt-get install rendercheck`;
   if it isn't packaged, build it from xorg/test/rendercheck. Run:
   `rendercheck -t tscoords,tmcoords,composite,cacomposite,repeat,blend -f a8r8g8b8,x8r8g8b8,a8`.
   Expected results:
   - Stock fails the transform tests (`tscoords`, `tmcoords`).
   - Patched with the option off passes everything.
   - Patched with the option on passes everything, except possibly
     single-pixel edge differences on bilinear tests (see above).
3. **Visual check in neomaster:** run it with each setting and compare cover
   edges, scaled covers and the rotating pointer beam. Rendering should match
   the `ExaTransform` off case, which is pixman.
4. **Speed:** compare neomaster's frame time while scrolling covers, with the
   option off and on.
5. **Stability:** a hang in the 3D engine freezes the box. Run step 2 first,
   over ssh, with the panel UI stopped.

## Upstream

The X driver has no active upstream. Kevin Brace's main branch was last
touched in 2023, and the freedesktop repos are archived. Patches 0001 and
0002 are plain bug fixes suitable for Debian (salsa.debian.org/xorg-team/driver/xserver-xorg-video-openchrome)
and the openchrome-devel list. 0001 deserves a Debian bug, since stock
bookworm openchrome can't start on any panel without EDID. 0003 should
follow once rendercheck passes on hardware.

`test/` holds the offline harness. Run
`test/run_harness.sh <unpatched-0.6.0-tree> <patched-tree>` with
`xserver-xorg-dev` and `libpixman-1-dev` installed. Both trees must be
configured first, so that `config.h` exists.
