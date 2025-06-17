install-cairo-lang:
	pip install cairo-lang==0.13.5

install-stwo:
	# NOTE: rust-toolchain.toml must be the same as the one in the stwo-cairo repo
	RUSTFLAGS="-C target-cpu=native -C opt-level=3" \
		cargo install \
		--git https://github.com/starkware-libs/stwo-cairo \
		--rev f8979ed82d86bd3408f9706a03a63c54bd221635 \
		adapted_stwo

install: install-cairo-lang install-stwo

submodules:
	git submodule update --init --recursive

compile:
	cairo-compile bootloader/bootloader.cairo \
		--output resources/stwo-bootloader.json \
		--cairo_path dependencies/cairo-lang/src \
		--proof_mode

execute:
	cargo run --release -- --pie examples/raito_1.zip --output-path examples/output

prove:
	adapted_stwo \
		--priv_json examples/output/priv.json \
		--pub_json examples/output/pub.json \
		--params_json prover_params.json \
		--proof_path examples/output/proof.json \
		--proof-format cairo-serde \
		--verify
