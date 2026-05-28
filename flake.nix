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
        cargoArtifacts = craneLib.buildDepsOnly commonArguments;
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
        packages.default = craneLib.buildPackage (commonArguments // { inherit cargoArtifacts; });
        apps.nix-integration-tests = {
          type = "app";
          program = "${nixIntegrationRunner}/bin/spirit-next-nix-integration-tests";
          meta.description = "Run Nix-built spirit-next integration tests";
        };
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
          generated-schema-source-checked-in = pkgs.runCommand "spirit-next-generated-schema-source-checked-in" { } ''
            test -f ${src}/src/schema/lib.rs
            grep -R "// @generated by schema-rust-next" ${src}/src/schema/lib.rs >/dev/null
            grep -R "SchemaPackage::new" ${src}/build.rs >/dev/null
            grep -R "SchemaEngine::default" ${src}/build.rs >/dev/null
            grep -R "lower_source_with_context" ${src}/build.rs >/dev/null
            grep -R "macros_applied" ${src}/build.rs >/dev/null
            grep -R "SchemaStructDefinition" ${src}/build.rs >/dev/null
            grep -R "SchemaEnumDefinition" ${src}/build.rs >/dev/null
            grep -R "RustEmitter::default().emit_file" ${src}/build.rs >/dev/null
            grep -R "schema/lib.schema" ${src}/build.rs >/dev/null
            grep -R "assert_checked_in_schema_is_fresh" ${src}/build.rs >/dev/null
            ! grep -R "fs::write" ${src}/build.rs
            ! grep -R "include!(concat!(env!(\"OUT_DIR\")" ${src}/src ${src}/build.rs
            grep -R "pub mod lib;" ${src}/src/lib.rs >/dev/null
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
          # Per record 1006 (Maximum, 2026-05-27): tests must PROVE not
          # pretend. The nix-integration test surface launches the
          # SAME schema-built binaries Nix produces, exchanging real
          # rkyv signal frames over a real Unix socket. This check
          # verifies the integration test file's anchors are intact
          # so future drift doesn't silently regress proof-shape.
          nix-integration-witness = pkgs.runCommand "spirit-next-nix-integration-witness" { } ''
            test -f ${src}/tests/nix_integration.rs
            test -x ${src}/scripts/run-nix-integration-tests
            grep -R "SPIRIT_NEXT_NIX_BUILD_RESULT" ${src}/tests/nix_integration.rs >/dev/null
            grep -R "SPIRIT_NEXT_NIX_BUILD_RESULT" ${src}/scripts/run-nix-integration-tests >/dev/null
            # Each test parses CLI stdout back through the schema-emitted
            # Output::FromStr, never asserting on raw strings.
            grep -R "Output::from_str" ${src}/tests/nix_integration.rs >/dev/null
            # The variant tour proves every schema-emitted Output variant
            # round-trips through CLI stdout intact.
            grep -R "nix_built_binaries_carry_schema_emitted_round_trip_for_every_output_variant" ${src}/tests/nix_integration.rs >/dev/null
            # The Rejected and Error variants are both exercised through
            # the binary boundary, not in-process.
            grep -R "Output::Rejected" ${src}/tests/nix_integration.rs >/dev/null
            grep -R "Output::Error" ${src}/tests/nix_integration.rs >/dev/null
            grep -R "Output::RecordAccepted" ${src}/tests/nix_integration.rs >/dev/null
            grep -R "Output::RecordsObserved" ${src}/tests/nix_integration.rs >/dev/null
            # The test uses the schema-emitted ValidationError variant.
            grep -R "ValidationError::EmptyTopic" ${src}/tests/nix_integration.rs >/dev/null
            # The test spawns the daemon and CLI binaries through the
            # Nix-built directory, not via CARGO_BIN_EXE (which would be
            # a cargo-built artifact, not Nix-built).
            grep -R "spirit_daemon" ${src}/tests/nix_integration.rs >/dev/null
            grep -R "spirit_cli" ${src}/tests/nix_integration.rs >/dev/null
            ! grep -R "CARGO_BIN_EXE" ${src}/tests/nix_integration.rs
            touch $out
          '';
          generated-signal-plane-used = pkgs.runCommand "spirit-next-generated-signal-plane-used" { } ''
            grep -R "InputRoute" ${src}/src/lib.rs >/dev/null
            grep -R "OutputRoute" ${src}/src/lib.rs >/dev/null
            grep -R "SignalFrameError" ${src}/src/lib.rs >/dev/null
            grep -R "NexusInput" ${src}/schema/lib.schema >/dev/null
            grep -R "NexusOutput" ${src}/schema/lib.schema >/dev/null
            grep -R "SemaInput" ${src}/schema/lib.schema >/dev/null
            grep -R "SemaOutput" ${src}/schema/lib.schema >/dev/null
            grep -R "Import \\[SourcePath LocalPath\\]" ${src}/schema/lib.schema >/dev/null
            grep -R "Export \\[LocalPath PublicPath\\]" ${src}/schema/lib.schema >/dev/null
            grep -R "Input::decode_signal_frame" ${src}/src/transport.rs >/dev/null
            grep -R "Output::decode_signal_frame" ${src}/src/transport.rs >/dev/null
            grep -R "input.encode_signal_frame" ${src}/src/transport.rs >/dev/null
            grep -R "output.encode_signal_frame" ${src}/src/transport.rs >/dev/null
            grep -R "MessageSent" ${src}/src/schema/lib.rs >/dev/null
            grep -R "OriginRoute" ${src}/src/schema/lib.rs >/dev/null
            grep -R "MessageRoot" ${src}/src/schema/lib.rs >/dev/null
            grep -R "NexusMail<Payload>" ${src}/src/schema/lib.rs >/dev/null
            grep -R "MessageProcessed<Reply>" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub mod schema" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub enum Plane" ${src}/src/schema/lib.rs >/dev/null
            grep -R "Sema(super::Sema<SemaRoot>)" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub mod signal" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub mod nexus" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub mod sema" ${src}/src/schema/lib.rs >/dev/null
            grep -R "MailLedgerEvent" ${src}/src/schema/lib.rs >/dev/null
            grep -R "DatabaseMarker" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub enum NexusInput" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub enum NexusOutput" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub enum SemaInput" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub enum SemaOutput" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub trait NexusEngine" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub trait SemaEngine" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub enum ValidationError" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub struct SignalRejection" ${src}/src/schema/lib.rs >/dev/null
            grep -R "pub struct SignalTransport" ${src}/src/transport.rs >/dev/null
            ! grep -R "pub enum InputRoute" ${src}/src/transport.rs
            ! grep -R "short_header::" ${src}/src/transport.rs
            grep -R "generated_input_surface_owns_route_header_and_rkyv_frame" ${src}/tests/generated_signal_plane.rs >/dev/null
            grep -R "generated_rejection_output_is_a_signal_schema_variant" ${src}/tests/generated_signal_plane.rs >/dev/null
            grep -R "generated_validation_error_round_trips_through_nota" ${src}/tests/generated_signal_plane.rs >/dev/null
            grep -R "generated_signal_surface_emits_mail_sent_event" ${src}/tests/generated_signal_plane.rs >/dev/null
            grep -R "event.origin_route()" ${src}/tests/generated_signal_plane.rs >/dev/null
            touch $out
          '';
          runtime-triad-visible = pkgs.runCommand "spirit-next-runtime-triad-visible" { } ''
            # Engine is a thin composer over Signal + Nexus; the SEMA call
            # lives INSIDE Nexus, never directly in Engine::handle.
            grep -R "signal_actor: SignalActor" ${src}/src/engine.rs >/dev/null
            grep -R "nexus: Mutex<Nexus>" ${src}/src/engine.rs >/dev/null
            grep -R "process_with" ${src}/src/engine.rs >/dev/null
            grep -R "into_being_processed" ${src}/src/engine.rs >/dev/null
            ! grep -R "self.store" ${src}/src/engine.rs
            ! grep -R "store.lock" ${src}/src/engine.rs
            grep -R "NexusMail<Entry>" ${src}/src/engine.rs >/dev/null
            grep -R "NexusMail<Query>" ${src}/src/engine.rs >/dev/null
            grep -R "MessageProcessed<Output>" ${src}/src/engine.rs >/dev/null
            grep -R "into_mail_ledger_event" ${src}/src/engine.rs >/dev/null
            grep -R "MailLedgerEvent" ${src}/src/engine.rs >/dev/null
            # Nexus is the real mail keeper: it owns the store + ledger and
            # holds the mail in a type-level BeingProcessed phase.
            grep -R "pub struct Nexus" ${src}/src/nexus.rs >/dev/null
            grep -R "store: Store" ${src}/src/nexus.rs >/dev/null
            grep -R "mail_ledger: MailLedger" ${src}/src/nexus.rs >/dev/null
            grep -R "pub struct Mail<Phase>" ${src}/src/nexus.rs >/dev/null
            grep -R "pub struct BeingProcessed" ${src}/src/nexus.rs >/dev/null
            grep -R "pub struct Processed" ${src}/src/nexus.rs >/dev/null
            grep -R "fn run_sema(self, store: &mut Store)" ${src}/src/nexus.rs >/dev/null
            grep -R "Mail<Processed>" ${src}/src/nexus.rs >/dev/null
            grep -R "impl NexusEngine for Nexus" ${src}/src/nexus.rs >/dev/null
            grep -R "into_nexus_output" ${src}/src/nexus.rs >/dev/null
            grep -R "into_signal_output" ${src}/src/nexus.rs >/dev/null
            grep -R "into_sema_input" ${src}/src/nexus.rs >/dev/null
            # SEMA is a durable redb database written to a .sema file.
            grep -R "DatabaseMarker" ${src}/src/store.rs >/dev/null
            grep -R "StateDigest" ${src}/src/store.rs >/dev/null
            grep -R "impl SemaEngine for Store" ${src}/src/store.rs >/dev/null
            grep -R "fn apply(" ${src}/src/store.rs >/dev/null
            grep -R "sema_plane::Sema<sema_plane::Input>" ${src}/src/store.rs >/dev/null
            grep -R "redb::" ${src}/src/store.rs >/dev/null
            grep -R "Database::create" ${src}/src/store.rs >/dev/null
            grep -R "Database::open" ${src}/src/store.rs >/dev/null
            grep -R "begin_write" ${src}/src/store.rs >/dev/null
            grep -R "begin_read" ${src}/src/store.rs >/dev/null
            grep -R "blake3" ${src}/src/store.rs >/dev/null
            ! grep -R "Vec<StoredRecord>" ${src}/src/store.rs
            ! grep -R "wrapping_mul" ${src}/src/store.rs
            # The .sema path fills an existing config position, no flag.
            grep -R "database_path" ${src}/src/config.rs >/dev/null
            # Tests prove the new topology + durability with schema objects.
            grep -R "nexus_holds_the_mail_in_being_processed_typestate_before_sema_runs" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "sema_store_persists_records_across_reopen_of_the_same_sema_file" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "sema_engine_writes_durable_records_and_returns_schema_objects" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "nexus_runs_sema_while_holding_mail_then_replies_through_schema_objects" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "signal_actor_rejects_invalid_input_with_schema_emitted_rejection_before_mail_or_sema" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "nexus_and_sema_have_explicit_input_output_languages" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "plane_envelopes_keep_payload_names_scoped" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "schema_meta::Plane::Sema" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "compile_fail" ${src}/src/lib.rs >/dev/null
            grep -R "nexus_plane::Nexus<nexus_plane::Input>" ${src}/src/lib.rs >/dev/null
            grep -R "import_export_paths_use_single_colon_namespaces" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "daemon_persists_sema_file_across_a_restart" ${src}/tests/process_boundary.rs >/dev/null
            grep -R "MailLedgerEvent::Sent" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "MailLedgerEvent::Processed" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "origin_route: route" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "in_flight.origin_route()" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "SemaEngine::apply" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "sent_message_count" ${src}/tests/runtime_triad.rs >/dev/null
            grep -R "processed_message_count" ${src}/tests/runtime_triad.rs >/dev/null
            ! grep -R "enum TraceEvent" ${src}/tests/runtime_triad.rs
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
