# OpenChrome X driver patches

Patches for Debian's `xserver-xorg-video-openchrome` (upstream 0.6.0), the X
driver the BM5's VIA CX700 uses on bookworm. Stock 0.6.0 crashes at start-up
on the BM5 under bookworm's Xorg 21.1, so patch 0001 is needed to get X
running at all. Neomaster's XRender backend
(`crates/render/src/xrender.rs`) scales cover art and rotates the pointer beam
with Render picture transforms and bilinear filtering. The stock driver gets
those wrong, and under UMS it never used the 3D engine for Render at all.
With the full series, Render composites run on the 3D engine. Background and the wider survey are in
[OpenChrome: unused acceleration on the CX700](https://claude.ai/artifact/KMuJdTNGui9KrX2vXBB76f).

| Patch | What it does | Default |
| --- | --- | --- |
| `0001` | Names the panel's native mode, which the driver generates itself because the BM5's panel has no EDID. Without a name, Xorg 21.1 segfaults in `xf86ProbeOutputModes` | Always on |
| `0002` | EXA composite falls back to software for anything the 3D engine can't draw: picture transforms, convolution filters, RepeatPad/Reflect, repeating pictures that aren't a power of two in size, and masks without a drawable | Always on |
| `0003` | Affine transforms (scale, rotate, skew) with nearest or bilinear filtering on the UniChrome 3D engine | Off; enable with `Option "ExaTransform" "true"` |
| `0004` | Under UMS, recognises offscreen pixmaps as video memory (`exaGetPixmapOffset`; the old check only knew the KMS front buffer, so every composite fell back to software). Lets `MigrationHeuristic` in xorg.conf replace the forced "greedy" | Always on; set `Option "MigrationHeuristic" "always"` |
| `0005` | Under UMS, gives EXA its own block of video memory. EXA pixmaps used to overlap the command queue, sync marker, cursors and Xv buffers | Always on |
| `0006` | H2 composites write destination alpha. Untransformed sources and masks without repeat are clipped to the picture, because xserver 21 no longer clips to the source drawable | Always on |
| `0007` | Saturates colour and alpha in the blend unit (Add wrapped around) | Always on |
| `0008` | Waits for both engines to go idle when the PCI command stream switches between 2D and 3D | Always on |
| `0009` | Breaks nearest-filter ties like pixman (2:1 downscales picked the other texel) | Always on |
| `0010` | Waits for the command queue to drain before each 3D segment in PCI mode. Without it, about 49 of 50 back-to-back composites were lost | Always on |
| `0011` | Sends transformed repeating pictures with the bilinear filter to software (the texture unit doesn't wrap bilinear samples) | Always on |
| `0012` | Draws Render solid-fill pictures (no drawable) as a constant colour, as source or mask, instead of falling back: text with a solid colour, tinted textures, images faded by a solid mask, translucent fills | Always on |
| `0013` | Picks the 2D engine mode for solid fills by bits per pixel, not depth. Depth 24 (the screen and window-format pixmaps) matched no mode, so every fill on them was done in software | Always on |

**Without 0002,** a transformed source over 400 pixels is drawn as an
unscaled crop of the image. Patches 0001 and 0002 make neomaster render
correctly, with software scaling. Patches 0004-0011 make composites reach
the 3D engine under UMS and draw correctly there. Patch 0003 moves the
scaling to the GPU as well.

Each patch's commit message says what was broken, how it was found, and the
fix. Debug-only builds (logging `PrepareComposite` calls and the VRAM layout)
were used along the way and are not included.

## Status

The full series runs on the BM5 (2026-11-28, as Debian package
`1:0.6.0-5+nm9`; 0001-0011 were `+nm8`) with `MigrationHeuristic "always"` and `ExaTransform "true"`.
X runs in UMS with EXA. The log shows
`[EXA] Offscreen pixmaps at 0x00641000-0x03800000 (50940 kB)` and
`EXA transformed and bilinear composites enabled`. Neomaster's Render use
goes to the 3D engine: the Add composites of 1024x320 and 90x768 and the
transformed bilinear 640x96.

`test/xformtest.c` compares X Render output with in-process pixman 0.42.2
over 318 cases (identity, 2x scale, 512->160 downscale, 17° rotation, 0.3
skew; nearest and bilinear; Over, Add and Src; with and without an a8 mask;
opaque, alpha and repeating sources). Results on the BM5 with
`MigrationHeuristic "always"`:

- **`ExaTransform` off:** 0 of 318 cases differ from pixman. Untransformed
  composites run on the 3D engine and are exact.
- **`ExaTransform` on:** no gross errors. 88 cases exceed the test's
  threshold (more than 2 levels off outside the edge band), all at precision
  level:
  - bilinear interiors are at most 13 levels off (the hardware's filter
    weights have less precision);
  - nearest with rotation, skew or a non-integer scale picks a neighbouring
    texel for under 1% of pixels (texture coordinate precision);
  - the bilinear edge band differs by up to half a pixel, as described under
    "How 0003 works".
- **droptest** (N back-to-back Add composites of 0x01): exact for N = 1..250.

Timings on the BM5's C7, in ms per composite:

| Composite | 3D engine (`ExaTransform` on) | Software (`ExaTransform` off) | pixman in-process |
| --- | --- | --- | --- |
| 512->160 bilinear Over | 0.64 | 1.36 | 1.29 |
| 512->160 nearest Over | 0.29 | 0.60 | 0.55 |
| 512->160 bilinear Src | 1.40 | 1.27 | 1.21 |
| 256x256 untransformed Over | 0.55 | 0.55 | |

Transformed Over runs about twice as fast as software. Transformed Src still
goes to software in both cases (0003 doesn't accept it for pictures without
repeat). The untransformed Over is on the 3D engine in both columns, at
about 120 Mpixel/s; in-process pixman takes 0.94 ms for it.

The offline harness (`test/run_harness.sh`) was written for 0003 and hasn't
been re-run on the full series. 0007 changes the blend setup words, so its
"untransformed: identical" check should be expected to report a difference
now; xformtest on hardware is the reference.

### Solid colours and fills (0012, 0013)

With 0001-0011 and `"always"`, a neomaster frame still mixed engines.
Logging every `CheckComposite` rejection, `PrepareSolid` failure and EXA
transfer (a debug build, not included) showed three kinds of fallback:
solid-fill sources and masks (text, the tinted beam, faded images),
glyph runs drawn through a temporary A8 picture that the 3D engine can't
render to, and every solid fill on a depth-24 pixmap (the
`viaAccelSetMode` depth/bpp mix-up). Each fallback made EXA copy the back
buffer out of video memory and back. During a 4 s pointer sweep that
was about 1,400 downloads and 2,800 uploads of parts of the 1024x768 buffer,
and about half of Xorg's time went to `memcpy`.

0012 and 0013 fix the driver side. Neomaster sends glyphs without a mask
format, so each glyph composites straight onto the back buffer
(`crates/render/src/xrender.rs`). With all three, the same sweep logs no
fallbacks and no transfers of the back buffer, only first-use glyph
uploads.

`test/solidtest.c` compares solid-fill composites and `FillRectangles` with
pixman on 24- and 32-bit destinations (56 cases: Over and Add with no mask,
an a8 mask, a rotated bilinear ARGB mask and a solid mask; images through a
solid mask; OutReverse; fills with Src, Over and Add). On the BM5 all 56 are
within 3 levels, the 3D blend's double rounding. Inside transformed bilinear
masks the limit is 14, as in xformtest, and their 2-pixel edge band is
skipped. xformtest still has the same 88 precision-level cases and droptest
is exact.

| Composite, 256x256, on the 3D engine | ms |
| --- | --- |
| Solid colour through a rotated bilinear ARGB mask, Add (the beam) | 0.13 |
| Solid colour through an a8 mask, Over (a glyph run) | 0.57 |
| Translucent solid colour, Over (a highlight bar) | 0.23 |

Neomaster's pointer sweep (neomaster#34's one-frame-in-flight renderer, one
panel report every 8 ms), input to screen:

| Driver and heuristic | Frames drawn | Median | p90 |
| --- | --- | --- | --- |
| nm8, `"always"` | 37 fps | 31 ms | 49 ms |
| nm8, `"greedy"` (all software) | 50 fps | 26 ms | 33 ms |
| nm9 (0012, 0013), `"always"` | every report (120/s) | 3.9 ms | 5.7 ms |

Xorg's CPU use during the sweep fell from about 70% to about 30%.

## Enabling on the BM5

Add both options to the Device section that X already uses. On the BM5
that is `Device "Card0"` in `/etc/X11/xorg.conf`; a separate file in
`xorg.conf.d` isn't picked up when xorg.conf already binds a Device to the
Screen.

```
Section "Device"
    Identifier "Card0"
    Driver     "openchrome"
    # ...existing options...
    Option     "MigrationHeuristic" "always"
    Option     "ExaTransform" "true"
EndSection
```

- `MigrationHeuristic "always"` needs 0004. Stock openchrome forces
  "greedy", and in xserver 21.1's classic EXA, greedy never moves pixmaps
  into video memory, so the 3D engine never sees a Render source.
- `ExaTransform "true"` enables 0003. Without it, transformed and bilinear
  composites go to software and the rest still uses the 3D engine.

Check the result with
`grep -E "EXA|Migration|transformed" /var/log/Xorg.0.log`.

**Rolling back:** the BM5 keeps a copy of the earlier config as
`/etc/X11/xorg.conf.pre-exatransform`. Remove the two options (or copy that
file back) and restart X for software rendering with the patched driver.
To drop the patches too, run
`sudo apt-get install --reinstall xserver-xorg-video-openchrome` and restart
X.

## Why acceleration never worked before

Stock openchrome 0.6.0 under UMS (no openchrome KMS module, as on the BM5)
never ran a Render composite on the 3D engine:

1. The driver forced `MigrationHeuristic "greedy"` and ignored the
   xorg.conf setting. Greedy never migrates pixmaps to video memory on
   xserver 21.1, so EXA fell back to software (0004).
2. Even with pixmaps in video memory, `viaExaIsOffscreen` only recognised
   the KMS front buffer object, so every pixmap still looked like system
   memory (0004).

That fallback hid a stack of bugs in the UMS 3D path, all of which showed up
once composites reached the engine:

3. EXA's pixmap area overlapped the command queue and the driver's other
   buffers, so composites drew in the wrong place or not at all (0005).
4. Destination alpha was never written, and RepeatNone sources weren't
   clipped now that xserver 21 doesn't clip to the source drawable (0006).
5. Add wasn't saturated (0007).
6. The PCI command path didn't wait between 2D blits and 3D composites
   (0008), and dropped 3D commands written while the queue was busy (0010).

Single composites can look right while bursts drop commands, so run the
droptest as well as the pixman comparison after any change.

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

Then enable the options as described under "Enabling on the BM5".

## Testing on hardware

Run tests over ssh with the panel UI stopped: a 3D engine hang freezes the
box.

1. **Driver mode.** `grep -E "EXA|Migration|transformed" /var/log/Xorg.0.log`
   should show the offscreen pixmap range, `MigrationHeuristic "always"` and
   "[EXA] Enabled EXA acceleration".
2. **xformtest.** Build it on the BM5:

   ```sh
   sudo apt-get install gcc libx11-dev libxrender-dev libpixman-1-dev pkg-config
   gcc -O2 -Wall -o xformtest xformtest.c $(pkg-config --cflags --libs x11 xrender pixman-1) -lm
   ```

   Then run `DISPLAY=:0 ./xformtest` and `DISPLAY=:0 ./xformtest --droptest`,
   with `ExaTransform` off and on. Expect the results under "Status". If the
   output is identical with the option on and off, nothing reached the 3D
   engine.
3. **rendercheck** (`-t tscoords,tmcoords,composite,cacomposite,repeat,blend`)
   is still useful for untransformed operators. Its transform tests use Src,
   so they never exercise 0003.

## Upstream

The X driver has no active upstream. Kevin Brace's main branch was last
touched in 2023, and the freedesktop repos are archived. Patches 0001,
0002 and 0004-0011 are plain bug fixes suitable for Debian (salsa.debian.org/xorg-team/driver/xserver-xorg-video-openchrome)
and the openchrome-devel list. 0001 deserves a Debian bug, since stock
bookworm openchrome can't start on any panel without EDID. 0003 has now
been checked against pixman on hardware and can follow.

`test/` holds the offline harness and `xformtest.c` (see "Testing on
hardware"). For the harness, run
`test/run_harness.sh <unpatched-0.6.0-tree> <patched-tree>` with
`xserver-xorg-dev` and `libpixman-1-dev` installed. Both trees must be
configured first, so that `config.h` exists.
