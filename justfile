size:
	cargo build --release
	cargo size --release -- -A | awk '/\.vector_table/ { v=$2 } /\.text/ { t=$2 } /\.rodata/ { r=$2 } END {print "FLASH SIZE used:" v+t+r}'

build:
    cargo build --release
    rust-objcopy --output-target=ihex target/thumbv6m-none-eabi/release/stm32l071_templates target/thumbv6m-none-eabi/release/stm32l071_templates.hex
    # rust-objcopy --output-target=binary target/thumbv6m-none-eabi/release/stm32l071_templates target/thumbv6m-none-eabi/release/stm32l071_templates.bin
    ./misc/hexcrc --fw-start=0x08001000 --fw-size=0xF000 --pm-start=0x08000000 --pm-size=0x10000 --pm-blocksize=4 --md-size=256 --gap-fill=0x00 \
        --btl-file=misc/stm32l0xx-bootloader.hex \
        --app-file=target/thumbv6m-none-eabi/release/stm32l071_templates.hex \
        --out-file=target/thumbv6m-none-eabi/release/unity-firmware.hex

# Recipe to flash the generated firmware using probe-rs
flash: build # Depends on the build recipe to ensure unity-firmware.hex exists
    @echo "Flashing device..."
    probe-rs download --chip=STM32L071C8Tx --binary-format=hex target/thumbv6m-none-eabi/release/unity-firmware.hex
    probe-rs reset --chip=STM32L071C8Tx
    @echo "Flashing completed successfully!"

# Optional: Recipe to erase the device
erase:
    @echo "Erasing device..."
    probe-rs erase --allow-erase-all --chip=STM32L071C8Tx
    @echo "Erase complete."

rtt: flash # Flash new FW with bootloader and debug
    @echo "Attaching RTT console..."
    probe-rs attach --chip=STM32L071C8Tx target/thumbv6m-none-eabi/release/stm32l071_templates