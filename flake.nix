{
  description = "spirit-next — runnable schema-derived Spirit pilot";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
    nota-next-source = {
      url = "github:LiGoldragon/nota-next";
      flake = false;
    };
    schema-next-source = {
      url = "github:LiGoldragon/schema-next";
      flake = false;
    };
    schema-rust-next-source = {
      url = "github:LiGoldragon/schema-rust-next";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, fenix, crane
    , nota-next-source
    , schema-next-source
    , schema-rust-next-source
  }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        toolchain = fenix.packages.${system}.stable.withComponents [
          "cargo"
          "rustc"
          "rustfmt"
          "clippy"
          "rust-src"
        ];
        craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
        schemaFilter = path: type:
          type == "regular" && pkgs.lib.hasSuffix ".schema" path;
        sourceFilter = path: type:
          (craneLib.filterCargoSources path type) || (schemaFilter path type);
        cleanSource = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = sourceFilter;
          name = "source";
        };
        src = pkgs.runCommand "spirit-next-source-with-local-schema-patches" {
          notaNextSource = nota-next-source;
          schemaNextSource = schema-next-source;
          schemaRustNextSource = schema-rust-next-source;
        } ''
          cp -R ${cleanSource} $out
          chmod -R u+w $out
          mkdir -p $out/vendor-sources
          cp -R "$notaNextSource" $out/vendor-sources/nota-next
          cp -R "$schemaNextSource" $out/vendor-sources/schema-next
          cp -R "$schemaRustNextSource" $out/vendor-sources/schema-rust-next

          cat >> $out/Cargo.toml <<'EOF'
          [patch."https://github.com/LiGoldragon/nota-next.git"]
          nota-next = { path = "vendor-sources/nota-next" }

          [patch."https://github.com/LiGoldragon/schema-next.git"]
          schema-next = { path = "vendor-sources/schema-next" }

          [patch."https://github.com/LiGoldragon/schema-rust-next.git"]
          schema-rust-next = { path = "vendor-sources/schema-rust-next" }
          EOF

          sed -i '\|^source = "git+https://github.com/LiGoldragon/nota-next.git?branch=main#|d' $out/Cargo.lock
          sed -i '\|^source = "git+https://github.com/LiGoldragon/schema-next.git?branch=main#|d' $out/Cargo.lock
          sed -i '\|^source = "git+https://github.com/LiGoldragon/schema-rust-next.git?branch=main#|d' $out/Cargo.lock
        '';
        cargoVendorDirectory = craneLib.vendorCargoDeps { inherit src; };
        commonArguments = {
          inherit src cargoVendorDirectory;
          strictDeps = true;
        };
        cargoArtifacts = craneLib.buildDepsOnly commonArguments;
      in
      {
        packages.default = craneLib.buildPackage (commonArguments // { inherit cargoArtifacts; });
        checks = {
          build = craneLib.cargoBuild (commonArguments // { inherit cargoArtifacts; });
          test = craneLib.cargoTest (commonArguments // { inherit cargoArtifacts; });
          no-old-signal-macro = pkgs.runCommand "spirit-next-no-old-signal-macro" { } ''
            if grep -R "signal_channel!" ${src}/build.rs ${src}/schema ${src}/src ${src}/tests; then
              echo "spirit-next must not use the old signal_channel macro" >&2
              exit 1
            fi
            touch $out
          '';
          generated-at-build-time = pkgs.runCommand "spirit-next-generated-at-build-time" { } ''
            grep -R "SchemaEngine::default" ${src}/build.rs >/dev/null
            grep -R "RustEmitter.emit_file" ${src}/build.rs >/dev/null
            grep -R "include!(concat!(env!(\"OUT_DIR\")" ${src}/src/lib.rs >/dev/null
            touch $out
          '';
          binary-boundary-test = pkgs.runCommand "spirit-next-binary-boundary-test" { } ''
            grep -R "encode_signal_frame" ${src}/src/transport.rs >/dev/null
            grep -R "decode_signal_frame" ${src}/src/transport.rs >/dev/null
            ! grep -R "rkyv::to_bytes" ${src}/src/transport.rs
            ! grep -R "rkyv::from_bytes" ${src}/src/transport.rs
            grep -R "Command::new(env!(\"CARGO_BIN_EXE_spirit-next\"))" ${src}/tests/process_boundary.rs >/dev/null
            touch $out
          '';
          generated-signal-plane-used = pkgs.runCommand "spirit-next-generated-signal-plane-used" { } ''
            grep -R "InputRoute" ${src}/src/lib.rs >/dev/null
            grep -R "OutputRoute" ${src}/src/lib.rs >/dev/null
            grep -R "SignalFrameError" ${src}/src/lib.rs >/dev/null
            grep -R "SemaCommand" ${src}/schema/spirit.schema >/dev/null
            grep -R "SemaResponse" ${src}/schema/spirit.schema >/dev/null
            grep -R "Input::decode_signal_frame" ${src}/src/transport.rs >/dev/null
            grep -R "Output::decode_signal_frame" ${src}/src/transport.rs >/dev/null
            grep -R "input.encode_signal_frame" ${src}/src/transport.rs >/dev/null
            grep -R "output.encode_signal_frame" ${src}/src/transport.rs >/dev/null
            ! grep -R "pub enum InputRoute" ${src}/src/transport.rs
            ! grep -R "short_header::" ${src}/src/transport.rs
            grep -R "generated_input_surface_owns_route_header_and_rkyv_frame" ${src}/tests/generated_signal_plane.rs >/dev/null
            touch $out
          '';
          runtime-triad-visible = pkgs.runCommand "spirit-next-runtime-triad-visible" { } ''
            grep -R "lower_to_sema" ${src}/src/engine.rs >/dev/null
            grep -R "SemaResponse" ${src}/src/engine.rs >/dev/null
            grep -R "pub fn apply(&mut self, command: SemaCommand)" ${src}/src/store.rs >/dev/null
            grep -R "executor_lowers_signal_input_to_generated_sema_command" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "sema_store_is_the_single_writer_for_records" ${src}/tests/runtime_triad.rs >/dev/null
            touch $out
          '';
          local-schema-source-patches = pkgs.runCommand "spirit-next-local-schema-source-patches" { } ''
            grep -R 'patch."https://github.com/LiGoldragon/nota-next.git"' ${src}/Cargo.toml >/dev/null
            grep -R 'patch."https://github.com/LiGoldragon/schema-next.git"' ${src}/Cargo.toml >/dev/null
            grep -R 'patch."https://github.com/LiGoldragon/schema-rust-next.git"' ${src}/Cargo.toml >/dev/null
            test -d ${src}/vendor-sources/nota-next
            test -d ${src}/vendor-sources/schema-next
            test -d ${src}/vendor-sources/schema-rust-next
            ! grep -R 'source = "git+https://github.com/LiGoldragon/nota-next.git' ${src}/Cargo.lock
            ! grep -R 'source = "git+https://github.com/LiGoldragon/schema-next.git' ${src}/Cargo.lock
            ! grep -R 'source = "git+https://github.com/LiGoldragon/schema-rust-next.git' ${src}/Cargo.lock
            touch $out
          '';
          fmt = craneLib.cargoFmt { inherit src; };
          clippy = craneLib.cargoClippy (commonArguments // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- -D warnings";
          });
          doc = craneLib.cargoDoc (commonArguments // {
            inherit cargoArtifacts;
            RUSTDOCFLAGS = "-D warnings";
          });
        };
        devShells.default = pkgs.mkShell {
          name = "spirit-next";
          packages = [ pkgs.jujutsu pkgs.pkg-config toolchain ];
        };
      });
}
