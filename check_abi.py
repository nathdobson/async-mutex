#!/usr/bin/python3
import subprocess

platforms = [
# install broken
#    "armebv7r-none-eabi",
#    "armebv7r-none-eabihf",
#    "armv5te-unknown-linux-gnueabi",
#    "armv5te-unknown-linux-musleabi",
#    "armv7a-none-eabi",
#    "armv7r-none-eabi",
#    "armv7r-none-eabihf",
#    "nvptx64-nvidia-cuda",
#    "thumbv7em-none-eabi",
#    "thumbv7em-none-eabihf",
#    "thumbv7m-none-eabi",
#    "thumbv8m.base-none-eabi",
#    "thumbv8m.main-none-eabi",
#    "thumbv8m.main-none-eabihf",
#    "thumbv6m-none-eabi",

# mips  missing DWCAS instruction
#    "mips-unknown-linux-gnu",
#    "mips-unknown-linux-musl",
#    "mips64-unknown-linux-gnuabi64",
#    "mips64-unknown-linux-muslabi64",
#    "mips64el-unknown-linux-gnuabi64",
#    "mips64el-unknown-linux-muslabi64",
#    "mipsel-unknown-linux-gnu",
#    "mipsel-unknown-linux-musl",

# Riscv missing DWCAS instruction
#    "riscv64gc-unknown-linux-gnu",
#    "riscv64gc-unknown-none-elf",
#    "riscv64imac-unknown-none-elf",
#    "riscv32i-unknown-none-elf",
#    "riscv32imac-unknown-none-elf",
#    "riscv32imc-unknown-none-elf",

# Power missing DWCAS instruction
#    "powerpc-unknown-linux-gnu",
#    "powerpc64-unknown-linux-gnu",
#    "powerpc64le-unknown-linux-gnu",

# Sparc missing DWCAS instruction
#    "sparc64-unknown-linux-gnu",
#    "sparcv9-sun-solaris",

# s390x missing DWCAS instruction
#    "s390x-unknown-linux-gnu",

# Missing DWCAS implementation, because of old AMD support?
#    "x86_64-pc-windows-gnu",
#    "x86_64-pc-windows-msvc",
#    "x86_64-unknown-linux-gnu",
#    "x86_64-unknown-linux-musl",
#    "x86_64-unknown-netbsd",
#    "x86_64-unknown-redox"
#    "x86_64-apple-ios",
#    "x86_64-fortanix-unknown-sgx",
#    "x86_64-fuchsia",
#    "x86_64-linux-android",
#    "x86_64-pc-solaris",
#    "x86_64-unknown-freebsd",
#    "x86_64-unknown-illumos",

# Missing DWCAS implementation, because of old armv5 support?
#    "aarch64-pc-windows-msvc",
#    "aarch64-unknown-none",
#    "aarch64-unknown-none-softfloat",
#    "arm-linux-androideabi",

    "aarch64-unknown-linux-gnu",
    "i686-pc-windows-gnu",
    "i686-pc-windows-msvc",
    "i686-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "aarch64-apple-ios",
    "aarch64-fuchsia",
    "aarch64-linux-android",
    "aarch64-unknown-linux-musl",
    "arm-unknown-linux-gnueabi",
    "arm-unknown-linux-gnueabihf",
    "arm-unknown-linux-musleabi",
    "arm-unknown-linux-musleabihf",
    "armv7-linux-androideabi",
    "armv7-unknown-linux-gnueabi",
    "armv7-unknown-linux-gnueabihf",
    "armv7-unknown-linux-musleabi",
    "armv7-unknown-linux-musleabihf",
    "asmjs-unknown-emscripten",
    "i586-pc-windows-msvc",
    "i586-unknown-linux-gnu",
    "i586-unknown-linux-musl",
    "i686-linux-android",
    "i686-unknown-freebsd",
    "i686-unknown-linux-musl",
    "thumbv7neon-linux-androideabi",
    "thumbv7neon-unknown-linux-gnueabihf",
    "wasm32-unknown-emscripten",
    "wasm32-unknown-unknown",
    "wasm32-wasi",
    "x86_64-unknown-linux-gnux32",
]

#subprocess.run(["rustup", "target", "add"] + platforms)
for platform in platforms:
    print("\nPLATFORM = " + platform)
    subprocess.run(["cargo", "check", "--target=" + platform, "--target-dir=target/platform-" + platform])
