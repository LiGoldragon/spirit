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
          type == "regular"
            && ((pkgs.lib.hasSuffix ".schema" path)
              || (pkgs.lib.hasSuffix ".asschema" path));
        # The scripts/ directory carries the workspace's harness for the
        # nix-driven integration tests (record 1006). Pull each script in
        # via a name match so the structural witness check + the script
        # itself are visible to Nix-built derivations.
        scriptFilter = path: type:
          (type == "regular" || type == "directory")
            && (builtins.match ".*/scripts(/.*)?" path != null);
        sourceFilter = path: type:
          (craneLib.filterCargoSources path type)
            || (schemaFilter path type)
            || (scriptFilter path type);
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
        binaryCargoArtifacts = craneLib.buildDepsOnly (commonArguments // {
          cargoExtraArgs = "--no-default-features";
        });
        notaTextCargoArtifacts = craneLib.buildDepsOnly (commonArguments // {
          cargoExtraArgs = "--features nota-text";
        });
        daemonPackage = craneLib.buildPackage (commonArguments // {
          cargoArtifacts = binaryCargoArtifacts;
          cargoExtraArgs = "--no-default-features --bin spirit-next-daemon";
        });
        cliPackage = craneLib.buildPackage (commonArguments // {
          cargoArtifacts = notaTextCargoArtifacts;
          cargoExtraArgs = "--features nota-text --bin spirit-next";
        });
        combinedPackage = pkgs.runCommand "spirit-next" { } ''
          mkdir -p "$out/bin"
          ln -s "${cliPackage}/bin/spirit-next" "$out/bin/spirit-next"
          ln -s "${daemonPackage}/bin/spirit-next-daemon" "$out/bin/spirit-next-daemon"
        '';
        nixIntegrationRunner = pkgs.writeShellApplication {
          name = "spirit-next-nix-integration-tests";
          runtimeInputs = [ pkgs.nix toolchain ];
          text = ''
            repo_root="''${SPIRIT_NEXT_REPO_ROOT:-$PWD}"
            exec "$repo_root/scripts/run-nix-integration-tests" "$@"
          '';
        };
      in
      {
        packages.default = combinedPackage;
        packages.cli = cliPackage;
        packages.daemon = daemonPackage;
        apps.nix-integration-tests = {
          type = "app";
          program = "${nixIntegrationRunner}/bin/spirit-next-nix-integration-tests";
          meta.description = "Run Nix-built spirit-next integration tests";
        };
        checks = {
          build = craneLib.cargoBuild (commonArguments // {
            cargoArtifacts = binaryCargoArtifacts;
            cargoExtraArgs = "--no-default-features";
          });
          build-nota-text = craneLib.cargoBuild (commonArguments // {
            cargoArtifacts = notaTextCargoArtifacts;
            cargoExtraArgs = "--features nota-text";
          });
          test = craneLib.cargoTest (commonArguments // {
            cargoArtifacts = binaryCargoArtifacts;
            cargoExtraArgs = "--no-default-features";
          });
          test-nota-text = craneLib.cargoTest (commonArguments // {
            cargoArtifacts = notaTextCargoArtifacts;
            cargoExtraArgs = "--features nota-text";
          });
          test-testing-trace = craneLib.cargoTest (commonArguments // {
            cargoArtifacts = binaryCargoArtifacts;
            cargoExtraArgs = "--features testing-trace --test instrumentation_logging";
          });
          no-old-signal-macro = pkgs.runCommand "spirit-next-no-old-signal-macro" { } ''
            if grep -R "signal_channel!" ${src}/build.rs ${src}/schema ${src}/src ${src}/tests; then
              echo "spirit-next must not use the old signal_channel macro" >&2
              exit 1
            fi
            touch $out
          '';
          generated-schema-source-checked-in = pkgs.runCommand "spirit-next-generated-schema-source-checked-in" { } ''
            test -f ${src}/src/schema/lib.rs
            test -f ${src}/schema/lib.asschema
            grep -R "// @generated by schema-rust-next" ${src}/src/schema/lib.rs >/dev/null
            grep -R "(Public Entry" ${src}/schema/lib.asschema >/dev/null
            grep -R "(Plain Entry)" ${src}/schema/lib.asschema >/dev/null
            grep -R "SchemaPackage::new" ${src}/build.rs >/dev/null
            grep -R "SchemaEngine::default" ${src}/build.rs >/dev/null
            grep -R "lower_source" ${src}/build.rs >/dev/null
            ! grep -R "lower_source_with_context" ${src}/build.rs
            ! grep -R "macros_applied" ${src}/build.rs
            ! grep -R "MacroContext" ${src}/build.rs
            ! grep -R "SchemaStructDefinition" ${src}/build.rs
            ! grep -R "SchemaEnumDefinition" ${src}/build.rs
            grep -R "AsschemaArtifact::new" ${src}/build.rs >/dev/null
            grep -R "write_nota_file" ${src}/build.rs >/dev/null
            grep -R "write_binary_file" ${src}/build.rs >/dev/null
            grep -R "emit_file_from_nota_path" ${src}/build.rs >/dev/null
            grep -R "emit_file_from_binary_path" ${src}/build.rs >/dev/null
            grep -R "RustEmissionOptions::feature_gated_nota(\"nota-text\")" ${src}/build.rs >/dev/null
            grep -R "RustEmitter::new" ${src}/build.rs >/dev/null
            grep -R "schema/lib.schema" ${src}/build.rs >/dev/null
            grep -R "lib.asschema" ${src}/build.rs >/dev/null
            grep -R "CheckedInAsschemaArtifact" ${src}/build.rs >/dev/null
            grep -R "assert_checked_in_schema_is_fresh" ${src}/build.rs >/dev/null
            grep -R "assert_matches_generated_artifact" ${src}/build.rs >/dev/null
            ! grep -R "fs::write" ${src}/build.rs
            ! grep -R "include!(concat!(env!(\"OUT_DIR\")" ${src}/src ${src}/build.rs
            grep -R "pub mod lib;" ${src}/src/lib.rs >/dev/null
            touch $out
          '';
          nota-surface-is-opt-in = pkgs.runCommand "spirit-next-nota-surface-is-opt-in" { } ''
            # Positive proof lives in tests/dependency_surface.rs, which
            # runs cargo tree for the binary-only and nota-text surfaces.
            # This check is only the negative guard for daemon-side text
            # decoder leakage.
            ! grep -R "nota_next" ${src}/src/config.rs ${src}/src/daemon.rs ${src}/src/bin/spirit-next-daemon.rs
            ! grep -R "NotaSource" ${src}/src/config.rs ${src}/src/daemon.rs ${src}/src/bin/spirit-next-daemon.rs
            touch $out
          '';
          binary-boundary-test = pkgs.runCommand "spirit-next-binary-boundary-test" { } ''
            # Positive proof lives in socket_negative.rs and
            # process_boundary.rs, which cross the real frame decoder and
            # Unix socket. This check only keeps transport from growing a
            # hand-written rkyv codec beside the generated frame methods.
            ! grep -R "rkyv::to_bytes" ${src}/src/transport.rs
            ! grep -R "rkyv::from_bytes" ${src}/src/transport.rs
            touch $out
          '';
          retired-triad-surfaces-absent = pkgs.runCommand "spirit-next-retired-triad-surfaces-absent" { } ''
            ! grep -R "pub struct Mail<Phase>" ${src}/src ${src}/tests
            ! grep -R "pub struct BeingProcessed" ${src}/src ${src}/tests
            ! grep -R "pub struct Processed" ${src}/src ${src}/tests
            ! grep -R "fn run_nexus(self, nexus: &mut Nexus)" ${src}/src ${src}/tests
            ! grep -R "FromMail" ${src}/src ${src}/tests
            ! grep -R "NexusMail<Payload>" ${src}/src ${src}/tests
            ! grep -R "InputNexus" ${src}/src ${src}/tests
            ! grep -R "OutputNexus" ${src}/src ${src}/tests
            ! grep -R "dispatch_mail_with_nexus" ${src}/src ${src}/tests
            ! grep -R "into_being_processed" ${src}/src ${src}/tests
            ! grep -R "into_sema_input" ${src}/src ${src}/tests
            ! grep -R "sema::Input" ${src}/src ${src}/tests
            ! grep -R "sema::Output" ${src}/src ${src}/tests
            touch $out
          '';
          no-production-free-functions = pkgs.runCommand "spirit-next-no-production-free-functions" { } ''
            if grep -R -n -E '^(pub(\([^)]*\))? )?fn ' ${src}/build.rs ${src}/src \
              | grep -v -E ':(fn main\()'; then
              echo "production Rust must not use module-level free functions except main" >&2
              exit 1
            fi
            touch $out
          '';
          no-production-unit-structs = pkgs.runCommand "spirit-next-no-production-unit-structs" { } ''
            if grep -R -n -E '^struct [A-Za-z][A-Za-z0-9_]*;' ${src}/src; then
              echo "production Rust must not use unit structs as namespace/method holders" >&2
              exit 1
            fi
            touch $out
          '';
          operator-271-closed-claims = pkgs.runCommand "spirit-next-operator-271-closed-claims" { } ''
            # Architectural-truth witnesses for operator 271 claim 4.
            # The Rust test file at tests/operator_271_closed_claims.rs runs
            # through cargo test; this Nix check verifies the witness names
            # are present and that the production schema source carries the
            # honest enum-body shape on the production daemon's path.
            test -f ${src}/tests/operator_271_closed_claims.rs
            # Claim 4 — honest enum bodies CLOSED.
            # The active production Input enum body — honest data variants.
            grep -F "[(Record Entry) (Observe Query) (Remove RecordIdentifier)]" ${src}/schema/lib.schema >/dev/null
            # The active production Output enum body.
            grep -F "(RecordAccepted SemaReceipt)" ${src}/schema/lib.schema >/dev/null
            grep -F "(RecordsObserved ObservedRecords)" ${src}/schema/lib.schema >/dev/null
            grep -F "(RecordRemoved RemoveReceipt)" ${src}/schema/lib.schema >/dev/null
            grep -F "(Error ErrorReport)" ${src}/schema/lib.schema >/dev/null
            grep -F "(Rejected SignalRejection)" ${src}/schema/lib.schema >/dev/null
            # Honest unit-variant bodies (bare PascalCase atoms).
            grep -F "ValidationError [EmptyTopic EmptyDescription EmptyQueryTopic]" ${src}/schema/lib.schema >/dev/null
            grep -F "Kind [Decision Principle Correction Clarification Constraint]" ${src}/schema/lib.schema >/dev/null
            grep -F "Magnitude [Minimum VeryLow Low Medium High VeryHigh Maximum]" ${src}/schema/lib.schema >/dev/null
            # Retired short-suffix sugar is absent from the production schema.
            if grep -F "@" ${src}/schema/lib.schema; then
              echo "spirit-next/schema/lib.schema must not carry the retired @ short-suffix sugar" >&2
              exit 1
            fi
            if grep -F "@" ${src}/schema/lib.asschema; then
              echo "spirit-next/schema/lib.asschema must not carry the retired @ short-suffix sugar" >&2
              exit 1
            fi
            # The assembled artifact lifts the honest source into the
            # typed (VariantName (Some (Plain TypeName))) form.
            grep -F "(Record (Some (Plain Entry)))" ${src}/schema/lib.asschema >/dev/null
            grep -F "(Observe (Some (Plain Query)))" ${src}/schema/lib.asschema >/dev/null
            grep -F "(EmptyTopic None)" ${src}/schema/lib.asschema >/dev/null
            grep -F "(Decision None)" ${src}/schema/lib.asschema >/dev/null
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
            cargoArtifacts = binaryCargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- -D warnings";
          });
          clippy-nota-text = craneLib.cargoClippy (commonArguments // {
            cargoArtifacts = notaTextCargoArtifacts;
            cargoClippyExtraArgs = "--features nota-text --all-targets -- -D warnings";
          });
          doc = craneLib.cargoDoc (commonArguments // {
            cargoArtifacts = binaryCargoArtifacts;
            RUSTDOCFLAGS = "-D warnings";
          });
        };
        devShells.default = pkgs.mkShell {
          name = "spirit-next";
          packages = [ pkgs.jujutsu pkgs.pkg-config toolchain ];
        };
      });
}
