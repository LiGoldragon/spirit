{
  description = "spirit — runnable schema-derived Spirit pilot";

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
    triad-runtime-source = {
      url = "github:LiGoldragon/triad-runtime";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, fenix, crane
    , nota-next-source
    , schema-next-source
    , schema-rust-next-source
    , triad-runtime-source
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
            && (pkgs.lib.hasSuffix ".schema" path);
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
        src = pkgs.runCommand "spirit-source-with-local-schema-patches" {
          notaNextSource = nota-next-source;
          schemaNextSource = schema-next-source;
          schemaRustNextSource = schema-rust-next-source;
          triadRuntimeSource = triad-runtime-source;
        } ''
          cp -R ${cleanSource} $out
          chmod -R u+w $out
          mkdir -p $out/vendor-sources
          cp -R "$notaNextSource" $out/vendor-sources/nota-next
          cp -R "$schemaNextSource" $out/vendor-sources/schema-next
          cp -R "$schemaRustNextSource" $out/vendor-sources/schema-rust-next
          cp -R "$triadRuntimeSource" $out/vendor-sources/triad-runtime

          substituteInPlace $out/Cargo.toml \
            --replace-fail 'nota-next = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "main", optional = true }' 'nota-next = { path = "vendor-sources/nota-next", optional = true }' \
            --replace-fail 'triad-runtime = { git = "https://github.com/LiGoldragon/triad-runtime.git", branch = "main" }' 'triad-runtime = { path = "vendor-sources/triad-runtime" }' \
            --replace-fail 'schema-rust-next = { git = "https://github.com/LiGoldragon/schema-rust-next.git", branch = "main" }' 'schema-rust-next = { path = "vendor-sources/schema-rust-next" }'

          substituteInPlace $out/vendor-sources/schema-rust-next/Cargo.toml \
            --replace-fail 'schema-next = { git = "https://github.com/LiGoldragon/schema-next.git", branch = "main" }' 'schema-next = { path = "../schema-next" }' \
            --replace-fail 'nota-next = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "main" }' 'nota-next = { path = "../nota-next" }'

          substituteInPlace $out/vendor-sources/schema-next/Cargo.toml \
            --replace-fail 'nota-next = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "main" }' 'nota-next = { path = "../nota-next" }'

          mkdir -p $out/.cargo
          cat >> $out/.cargo/config.toml <<'EOF'
          paths = [
            "vendor-sources/nota-next",
            "vendor-sources/nota-next/derive",
            "vendor-sources/schema-next",
            "vendor-sources/schema-rust-next",
            "vendor-sources/triad-runtime",
          ]
          EOF

          cat >> $out/Cargo.toml <<'EOF'
          [patch."https://github.com/LiGoldragon/nota-next.git?branch=main"]
          nota-next = { path = "vendor-sources/nota-next" }
          nota-next-derive = { path = "vendor-sources/nota-next/derive" }

          [patch."https://github.com/LiGoldragon/schema-next.git?branch=main"]
          schema-next = { path = "vendor-sources/schema-next" }

          [patch."https://github.com/LiGoldragon/schema-rust-next.git?branch=main"]
          schema-rust-next = { path = "vendor-sources/schema-rust-next" }

          [patch."https://github.com/LiGoldragon/triad-runtime.git?branch=main"]
          triad-runtime = { path = "vendor-sources/triad-runtime" }
          EOF

          sed -i '\|^source = "git+https://github.com/LiGoldragon/nota-next.git?branch=main#|d' $out/Cargo.lock
          sed -i '\|^source = "git+https://github.com/LiGoldragon/schema-next.git?branch=main#|d' $out/Cargo.lock
          sed -i '\|^source = "git+https://github.com/LiGoldragon/schema-rust-next.git?branch=main#|d' $out/Cargo.lock
          sed -i '\|^source = "git+https://github.com/LiGoldragon/triad-runtime.git?branch=main#|d' $out/Cargo.lock
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
        testingTraceCargoArtifacts = craneLib.buildDepsOnly (commonArguments // {
          cargoExtraArgs = "--features testing-trace";
        });
        notaTextTestingTraceCargoArtifacts = craneLib.buildDepsOnly (commonArguments // {
          cargoExtraArgs = "--features nota-text,testing-trace";
        });
        daemonPackage = craneLib.buildPackage (commonArguments // {
          cargoArtifacts = binaryCargoArtifacts;
          cargoExtraArgs = "--no-default-features --bin spirit-daemon";
        });
        cliPackage = craneLib.buildPackage (commonArguments // {
          cargoArtifacts = notaTextCargoArtifacts;
          cargoExtraArgs = "--features nota-text --bin spirit";
        });
        traceDaemonPackage = craneLib.buildPackage (commonArguments // {
          cargoArtifacts = testingTraceCargoArtifacts;
          cargoExtraArgs = "--features testing-trace --bin spirit-daemon";
        });
        traceCliPackage = craneLib.buildPackage (commonArguments // {
          cargoArtifacts = notaTextTestingTraceCargoArtifacts;
          cargoExtraArgs = "--features nota-text,testing-trace --bin spirit";
        });
        combinedPackage = pkgs.runCommand "spirit" { } ''
          mkdir -p "$out/bin"
          ln -s "${cliPackage}/bin/spirit" "$out/bin/spirit"
          ln -s "${daemonPackage}/bin/spirit-daemon" "$out/bin/spirit-daemon"
        '';
        traceCombinedPackage = pkgs.runCommand "spirit-trace" { } ''
          mkdir -p "$out/bin"
          ln -s "${traceCliPackage}/bin/spirit" "$out/bin/spirit"
          ln -s "${traceDaemonPackage}/bin/spirit-daemon" "$out/bin/spirit-daemon"
        '';
        nixIntegrationRunner = pkgs.writeShellApplication {
          name = "spirit-nix-integration-tests";
          runtimeInputs = [ pkgs.nix toolchain ];
          text = ''
            repo_root="''${SPIRIT_REPO_ROOT:-$PWD}"
            exec "$repo_root/scripts/run-nix-integration-tests" "$@"
          '';
        };
      in
      {
        packages.default = combinedPackage;
        packages.cli = cliPackage;
        packages.daemon = daemonPackage;
        packages.trace = traceCombinedPackage;
        packages."trace-cli" = traceCliPackage;
        packages."trace-daemon" = traceDaemonPackage;
        apps.nix-integration-tests = {
          type = "app";
          program = "${nixIntegrationRunner}/bin/spirit-nix-integration-tests";
          meta.description = "Run Nix-built spirit integration tests";
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
            cargoArtifacts = testingTraceCargoArtifacts;
            cargoExtraArgs = "--features testing-trace --test instrumentation_logging";
          });
          test-testing-trace-process-boundary = craneLib.cargoTest (commonArguments // {
            cargoArtifacts = notaTextTestingTraceCargoArtifacts;
            cargoExtraArgs = "--features nota-text,testing-trace --test process_boundary cli_receives_testing_trace_events_from_daemon_trace_socket -- --exact";
          });
          no-old-signal-macro = pkgs.runCommand "spirit-no-old-signal-macro" { } ''
            if grep -R "signal_channel!" ${src}/build.rs ${src}/schema ${src}/src ${src}/tests; then
              echo "spirit must not use the old signal_channel macro" >&2
              exit 1
            fi
            touch $out
          '';
          generated-schema-source-checked-in = pkgs.runCommand "spirit-generated-schema-source-checked-in" { } ''
            # Positive freshness proof runs through the cargo build/test
            # checks: build.rs decodes each plane schema as SchemaSource,
            # round-trips canonical source and rkyv archive values, emits
            # Rust from the typed schema-in-Rust values, and compares the
            # checked-in generated Rust. This check only keeps retired
            # side-channel source paths absent.
            test -f ${src}/src/schema/signal.rs
            test -f ${src}/src/schema/nexus.rs
            test -f ${src}/src/schema/sema.rs
            ! grep -R "lower_source(" ${src}/build.rs
            ! grep -R "lower_source_with_context" ${src}/build.rs
            ! grep -R "macros_applied" ${src}/build.rs
            ! grep -R "MacroContext" ${src}/build.rs
            ! grep -R "SchemaStructDefinition" ${src}/build.rs
            ! grep -R "SchemaEnumDefinition" ${src}/build.rs
            ! grep -R "include!(concat!(env!(\"OUT_DIR\")" ${src}/src ${src}/build.rs
            touch $out
          '';
          nota-surface-is-opt-in = pkgs.runCommand "spirit-nota-surface-is-opt-in" { } ''
            # Positive proof lives in tests/dependency_surface.rs, which
            # runs cargo tree for the binary-only and nota-text surfaces.
            # This check is only the negative guard for daemon-side text
            # decoder leakage.
            ! grep -R "nota_next" ${src}/src/config.rs ${src}/src/daemon.rs ${src}/src/bin/spirit-daemon.rs
            ! grep -R "NotaSource" ${src}/src/config.rs ${src}/src/daemon.rs ${src}/src/bin/spirit-daemon.rs
            touch $out
          '';
          binary-boundary-test = pkgs.runCommand "spirit-binary-boundary-test" { } ''
            # Positive proof lives in socket_negative.rs and
            # process_boundary.rs, which cross the real frame decoder and
            # Unix socket. This check only keeps transport from growing a
            # hand-written rkyv codec beside the generated frame methods.
            ! grep -R "rkyv::to_bytes" ${src}/src/transport.rs
            ! grep -R "rkyv::from_bytes" ${src}/src/transport.rs
            touch $out
          '';
          retired-triad-surfaces-absent = pkgs.runCommand "spirit-retired-triad-surfaces-absent" { } ''
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
          no-production-free-functions = pkgs.runCommand "spirit-no-production-free-functions" { } ''
            if grep -R -n -E '^(pub(\([^)]*\))? )?fn ' ${src}/build.rs ${src}/src \
              | grep -v -E ':(fn main\()'; then
              echo "production Rust must not use module-level free functions except main" >&2
              exit 1
            fi
            touch $out
          '';
          no-production-unit-structs = pkgs.runCommand "spirit-no-production-unit-structs" { } ''
            if grep -R -n -E '^struct [A-Za-z][A-Za-z0-9_]*;' ${src}/src; then
              echo "production Rust must not use unit structs as namespace/method holders" >&2
              exit 1
            fi
            touch $out
          '';
          operator-271-closed-claims = craneLib.cargoTest (commonArguments // {
            cargoArtifacts = binaryCargoArtifacts;
            cargoExtraArgs = "--no-default-features --test operator_271_closed_claims";
          });
          fmt = craneLib.cargoFmt { inherit src; };
          clippy = craneLib.cargoClippy (commonArguments // {
            cargoArtifacts = binaryCargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- -D warnings";
          });
          clippy-nota-text = craneLib.cargoClippy (commonArguments // {
            cargoArtifacts = notaTextCargoArtifacts;
            cargoClippyExtraArgs = "--features nota-text --all-targets -- -D warnings";
          });
          clippy-testing-trace = craneLib.cargoClippy (commonArguments // {
            cargoArtifacts = notaTextTestingTraceCargoArtifacts;
            cargoClippyExtraArgs = "--features nota-text,testing-trace --all-targets -- -D warnings";
          });
          doc = craneLib.cargoDoc (commonArguments // {
            cargoArtifacts = binaryCargoArtifacts;
            RUSTDOCFLAGS = "-D warnings";
          });
        };
        devShells.default = pkgs.mkShell {
          name = "spirit";
          packages = [ pkgs.jujutsu pkgs.pkg-config toolchain ];
        };
      });
}
