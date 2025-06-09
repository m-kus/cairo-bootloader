install-stwo:
	# NOTE: rust-toolchain.toml must be the same as the one in the stwo-cairo repo
	RUSTFLAGS="-C target-cpu=native -C opt-level=3" \
		cargo install \
		--git https://github.com/starkware-libs/stwo-cairo \
		--rev c3effb7a5b6f212bed14361618562a0a1007f86b \
		adapted_stwo

compile:
	./scripts/compile-bootloader.sh

execute:
	cargo run --release

prove:
	adapted_stwo \
		--priv_json examples/output/priv.json \
		--pub_json examples/output/pub.json \
		--params_json prover_params.json \
		--proof_path examples/output/proof.json \
		--proof-format cairo-serde \
		--verify
