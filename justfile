flash-size:
	cargo build --release
	cargo size --release -- -A | awk '/\.vector_table/ { v=$2 } /\.text/ { t=$2 } /\.rodata/ { r=$2 } END {print "FLASH SIZE used:" v+t+r}'

