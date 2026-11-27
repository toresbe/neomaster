opus-rs 0.1.23 from crates.io, patched for 32-bit x86 (the BeoMaster 5 is i686).

Upstream imports `std::arch::x86_64::*` in code that is also compiled for
`target_arch = "x86"`, which fails to build there. Each such import is now
`std::arch::x86::*` on x86 and `std::arch::x86_64::*` on x86-64; nothing
else is changed. Benches, tests, examples and fixtures were dropped.

It is only here because `sendspin` depends on it unconditionally; the
player offers MA PCM only, so Opus is never decoded. Remove this directory
and the `[patch.crates-io]` entry once upstream builds on i686.
