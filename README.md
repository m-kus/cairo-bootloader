# Cairo bootloader

Cairo bootloader port for the Rust Cairo VM.

The Cairo bootloader is a Cairo program that loads and executes other programs in a provable way.
It is also able to execute Cairo PIEs (Position Independent Executables) along with regular Cairo programs.

We currently support Cairo bootloader v0.13.1.

## Installation

```sh
cargo install --git https://github.com/m-kus/cairo-bootloader --rev 0861070b85cac2f4425cfed35fc2a401291bddd5 cairo-bootloader
```

## Usage

Generate PIE using [`cairo-execute`](https://github.com/m-kus/cairo/pull/4) (note that Stwo is compatible with a specific cairo-vm commit).

```sh
stwo-bootloader --pie <path-to-the-pie> --output-path <output-dir>
```

In the output directory you will find memory/trace binary files as well as public/private input JSON files.  
Compatible with `adapted_stwo` prover binary.
