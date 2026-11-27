# neomaster
Alternate software for the BeoMaster 5

**neomaster 2 (Rust)** is being written alongside the C++ code: a Cargo workspace in `crates/`
with libpc2's Masterlink and PC2 layers ported (their capture replays pass byte for byte), a panel
driver, an XRender UI, and `neomaster --sim`, which runs the panel UI in a desktop window. See
[doc/rust-workspace.md](doc/rust-workspace.md) for the layout, how to cross-build for the BM5
(i686) and what is ported.

This project serves two functions:

1. Improve my C++ (so, please don't judge the current code quality. Constructive criticism very welcome.)
2. Replace the excellent but limited software on the Beosound/Beomaster 5, particularily to re-enable Spotify

I will

1. ~~Reverse engineer the basics of controlling the Beosound 5 panel~~ Done :)
2. ~~Reverse engineer the PC2 in Audio Master mode~~ This is done, and separated off to the libpc2 library
3. Create a simple example module to control my Philips Hue lights (to have some functionality that does not require reverse engineering the PC2)
4. Create a basic user interface, controlled by the Beosound 5 panel
5. Play back media under panel control and output sound via the PC2.
6. ~~Obtain IR data from the panel sensor/PC2 and control media with it~~ Done. See the libpc2 library. Note the API is frequently changing.
7. ~~Get Spotify back~~ Done, but still _very_ hackish.

Future questions to be resolved include:

1. Can the B&O PC2 of all revisions be configured to operate as an Audio Master in a Beolink system?
2. ~~How does the IR sensor on the Beosound 5 panel actually send its signal?~~ It uses a spare pin on its power wire(!)
