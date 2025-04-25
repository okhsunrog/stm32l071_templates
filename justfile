size:
	cargo build --release
	cargo size --release -- -A | awk '/\.vector_table/ { v=$2 } /\.text/ { t=$2 } /\.rodata/ { r=$2 } END {print "FLASH SIZE used:" v+t+r}'

build:
    cargo build --release
    rust-objcopy --output-target=ihex target/thumbv6m-none-eabi/release/hello target/thumbv6m-none-eabi/release/hello.hex
    rust-objcopy --output-target=binary target/thumbv6m-none-eabi/release/hello target/thumbv6m-none-eabi/release/hello.bin
    ./scripts/hexcrc --fw-start=0x08001000 --fw-size=0xF000 --pm-start=0x08000000 --pm-size=0x10000 --pm-blocksize=4 --md-size=256 --gap-fill=0x00 \
        --btl-file=/home/okhsunrog/stm32l0xx-bootloader.hex \
        --app-file=target/thumbv6m-none-eabi/release/hello.hex \
        --out-file=unity-firmware.hex

