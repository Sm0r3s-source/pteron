# Building Pteron

Pteron is a set of files laid over a [Servo](https://github.com/servo/servo) **v0.7.0** checkout: new files under
`ports/servoshell/`, and changed copies of some of Servo's own files under `components/`, `python/` and `resources/`.
This repository holds only those files, at the same paths they have in Servo. The scripts in `pteron_build/` fetch
Servo v0.7.0, copy the files over it and build.

## Linux (Ubuntu 24.04, or WSL2)

```
git clone https://github.com/Sm0r3s-source/pteron.git
bash pteron/pteron_build/from_repo.sh
```

About 25 GB of disk and 8 GB of memory. The script installs Rust and `uv` if they are missing and runs Servo's own
`./mach bootstrap` for the system packages. `--no-compat` leaves out the compatibility tab (no WebKitGTK needed).

## Windows

Set up what Servo needs first:
- Rust (the toolchain Servo pins is installed by itself on the first build)
- Visual Studio Build Tools with the "Desktop development with C++" workload
- LLVM (`winget install LLVM.LLVM`), so that `lld-link.exe` is on your PATH
- The other prerequisites in the [Servo book](https://book.servo.org)

```
git clone https://github.com/Sm0r3s-source/pteron.git
powershell -ExecutionPolicy Bypass -File .\pteron\pteron_build\from_repo.ps1
```

`-Production` builds like the official Servo releases (link-time optimisation, optimised for size): a smaller, faster
Pteron that uses less memory, but the build takes an hour or more and needs about 16 GB of memory.

## By hand

```
git clone --depth 1 --branch v0.7.0 https://github.com/servo/servo.git
cp -r pteron/components pteron/ports pteron/python pteron/resources servo/
echo AAAA > servo/ports/servoshell/update_key.pub
cd servo && ./mach build -r --features compat_linux
```

## The update key

Official releases have the author's public update key built in (`ports/servoshell/update_key.pub`, made on the
author's PC and not in this repository). A build without it gets a placeholder: everything works except that it
refuses to install updates by itself, so rebuild to update.

## Seeing exactly what Pteron changes in Servo

After the files are copied, the Servo checkout shows it:

```
cd servo-v0.7.0
git status --short          # new files (??) and changed Servo files (M)
git diff -- components      # every change to Servo's own engine files
```

## Tests

`pteron_ci/` runs the unit tests of Pteron's own modules without building Servo
(`cargo test --manifest-path pteron_ci/Cargo.toml`, with any text in `ports/servoshell/update_key.pub`). They run on
every push.

## How releases are made

The author builds on Windows and in WSL2 with update scripts that do the same copy-and-build, then pack the installer,
zip and Linux tarball, sign their SHA-256 with the update key, and publish them. `BUILD.txt` names the build number
and the Servo version of the files in this repository.
