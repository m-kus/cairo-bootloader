install-stwo:
	# NOTE: rust-toolchain.toml must be the same as the one in the stwo-cairo repo
	RUSTFLAGS="-C target-cpu=native -C opt-level=3" \
		cargo install \
		--git https://github.com/starkware-libs/stwo-cairo \
		--rev 48a05a3bceb579382bfaefb6dcf08a53d9e175bc \
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
