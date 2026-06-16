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
      url = "github:LiGoldragon/nota-next/structural-forms-integration";
      flake = false;
    };
    schema-next-source = {
      url = "github:LiGoldragon/schema-next/structural-forms-integration";
      flake = false;
    };
    schema-rust-next-source = {
      url = "github:LiGoldragon/schema-rust-next/structural-forms-integration";
      flake = false;
    };
    sema-source = {
      url = "github:LiGoldragon/sema";
      flake = false;
    };
    sema-engine-source = {
      url = "github:LiGoldragon/sema-engine/structural-forms-integration";
      flake = false;
    };
    # The previous engine generation: reads pre-versioning stores for the
    # production-migration bootstrap.
    sema-engine-previous-source = {
      url = "github:LiGoldragon/sema-engine/ebee6e44ba6ee4afcb26998007bcfd128641b54c";
      flake = false;
    };
    # The deployed v9/layout-3 engine generation: reads materialized rows
    # from the live 0.12.1 store for the production-migration bootstrap.
    sema-engine-layout3-source = {
      url = "github:LiGoldragon/sema-engine/dbe29427d9a2c6c194909385485ad42b008048b8";
      flake = false;
    };
    signal-frame-source = {
      url = "github:LiGoldragon/signal-frame";
      flake = false;
    };
    signal-sema-source = {
      url = "github:LiGoldragon/signal-sema";
      flake = false;
    };
    triad-runtime-source = {
      url = "github:LiGoldragon/triad-runtime/structural-forms-integration";
      flake = false;
    };
    signal-spirit-source = {
      url = "github:LiGoldragon/signal-spirit/structural-forms-integration";
      flake = false;
    };
    meta-signal-spirit-source = {
      url = "github:LiGoldragon/meta-signal-spirit/structural-forms-integration";
      flake = false;
    };
    signal-agent-source = {
      url = "github:LiGoldragon/signal-agent";
      flake = false;
    };
    meta-signal-agent-source = {
      url = "github:LiGoldragon/meta-signal-agent";
      flake = false;
    };
    agent-source = {
      url = "github:LiGoldragon/agent";
      flake = false;
    };
    version-projection-source = {
      url = "github:LiGoldragon/version-projection";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      fenix,
      crane,
      nota-next-source,
      schema-next-source,
      schema-rust-next-source,
      sema-source,
      sema-engine-source,
      sema-engine-previous-source,
      sema-engine-layout3-source,
      signal-frame-source,
      signal-sema-source,
      triad-runtime-source,
      signal-spirit-source,
      meta-signal-spirit-source,
      signal-agent-source,
      meta-signal-agent-source,
      agent-source,
      version-projection-source,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
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
        schemaFilter = path: type: type == "regular" && (pkgs.lib.hasSuffix ".schema" path);
        # The scripts/ directory carries the workspace's harness for the
        # nix-driven integration tests (record 1006). Pull each script in
        # via a name match so the structural witness check + the script
        # itself are visible to Nix-built derivations.
        scriptFilter =
          path: type:
          (type == "regular" || type == "directory") && (builtins.match ".*/scripts(/.*)?" path != null);
        # The guardian prompt prose lives in src/guardian-prompts/*.md and is
        # pulled into the daemon with include_str!; the crane cargo-source filter
        # drops .md, so pull the directory and its files in by name match or the
        # build fails to read them.
        promptFilter =
          path: type:
          (type == "regular" || type == "directory")
          && (builtins.match ".*/guardian-prompts(/.*)?" path != null);
        sourceFilter =
          path: type:
          (craneLib.filterCargoSources path type)
          || (schemaFilter path type)
          || (scriptFilter path type)
          || (promptFilter path type);
        cleanSource = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = sourceFilter;
          name = "source";
        };
        src =
          pkgs.runCommand "spirit-source-with-local-schema-patches"
            {
              notaNextSource = nota-next-source;
              schemaNextSource = schema-next-source;
              schemaRustNextSource = schema-rust-next-source;
              semaSource = sema-source;
              semaEngineSource = sema-engine-source;
              semaEnginePreviousSource = sema-engine-previous-source;
              semaEngineLayout3Source = sema-engine-layout3-source;
              signalFrameSource = signal-frame-source;
              signalSemaSource = signal-sema-source;
              triadRuntimeSource = triad-runtime-source;
              signalSpiritSource = signal-spirit-source;
              metaSignalSpiritSource = meta-signal-spirit-source;
              signalAgentSource = signal-agent-source;
              metaSignalAgentSource = meta-signal-agent-source;
              agentSource = agent-source;
              versionProjectionSource = version-projection-source;
            }
            ''
              cp -R ${cleanSource} $out
              chmod -R u+w $out
              mkdir -p $out/vendor-sources
              cp -R "$notaNextSource" $out/vendor-sources/nota-next
              cp -R "$schemaNextSource" $out/vendor-sources/schema-next
              cp -R "$schemaRustNextSource" $out/vendor-sources/schema-rust-next
              cp -R "$semaSource" $out/vendor-sources/sema
              cp -R "$semaEngineSource" $out/vendor-sources/sema-engine
              cp -R "$semaEnginePreviousSource" $out/vendor-sources/sema-engine-previous
              cp -R "$semaEngineLayout3Source" $out/vendor-sources/sema-engine-layout3
              cp -R "$signalFrameSource" $out/vendor-sources/signal-frame
              cp -R "$signalSemaSource" $out/vendor-sources/signal-sema
              cp -R "$triadRuntimeSource" $out/vendor-sources/triad-runtime
              cp -R "$signalSpiritSource" $out/vendor-sources/signal-spirit
              cp -R "$metaSignalSpiritSource" $out/vendor-sources/meta-signal-spirit
              cp -R "$signalAgentSource" $out/vendor-sources/signal-agent
              cp -R "$metaSignalAgentSource" $out/vendor-sources/meta-signal-agent
              cp -R "$agentSource" $out/vendor-sources/agent
              cp -R "$versionProjectionSource" $out/vendor-sources/version-projection

              substituteInPlace $out/Cargo.toml \
                --replace-fail 'nota-next = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "structural-forms-integration", optional = true }' 'nota-next = { path = "vendor-sources/nota-next", optional = true }' \
                --replace-fail 'sema-engine = { git = "https://github.com/LiGoldragon/sema-engine.git", branch = "structural-forms-integration" }' 'sema-engine = { path = "vendor-sources/sema-engine" }' \
                --replace-fail 'sema-engine-previous = { git = "https://github.com/LiGoldragon/sema-engine.git", rev = "ebee6e44ba6ee4afcb26998007bcfd128641b54c", package = "sema-engine", optional = true }' 'sema-engine-previous = { path = "vendor-sources/sema-engine-previous", package = "sema-engine", optional = true }' \
                --replace-fail 'sema-engine-layout3 = { git = "https://github.com/LiGoldragon/sema-engine.git", rev = "dbe29427d9a2c6c194909385485ad42b008048b8", package = "sema-engine", optional = true }' 'sema-engine-layout3 = { path = "vendor-sources/sema-engine-layout3", package = "sema-engine", optional = true }' \
                --replace-fail 'signal-frame = { git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main" }' 'signal-frame = { path = "vendor-sources/signal-frame" }' \
                --replace-fail 'signal-agent = { git = "https://github.com/LiGoldragon/signal-agent.git", branch = "main", optional = true }' 'signal-agent = { path = "vendor-sources/signal-agent", optional = true }' \
                --replace-fail 'signal-sema = { git = "https://github.com/LiGoldragon/signal-sema.git", branch = "main" }' 'signal-sema = { path = "vendor-sources/signal-sema" }' \
                --replace-fail 'signal-spirit = { git = "https://github.com/LiGoldragon/signal-spirit.git", branch = "structural-forms-integration" }' 'signal-spirit = { path = "vendor-sources/signal-spirit" }' \
                --replace-fail 'meta-signal-spirit = { git = "https://github.com/LiGoldragon/meta-signal-spirit.git", branch = "structural-forms-integration" }' 'meta-signal-spirit = { path = "vendor-sources/meta-signal-spirit" }' \
                --replace-fail 'triad-runtime = { git = "https://github.com/LiGoldragon/triad-runtime.git", branch = "structural-forms-integration" }' 'triad-runtime = { path = "vendor-sources/triad-runtime" }' \
                --replace-fail 'schema-rust-next = { git = "https://github.com/LiGoldragon/schema-rust-next.git", branch = "structural-forms-integration" }' 'schema-rust-next = { path = "vendor-sources/schema-rust-next" }' \
                --replace-fail 'agent = { git = "https://github.com/LiGoldragon/agent.git", branch = "main", features = ["live-provider"] }' 'agent = { path = "vendor-sources/agent", features = ["live-provider"] }' \
                --replace-fail 'schema-next = { git = "https://github.com/LiGoldragon/schema-next.git", branch = "structural-forms-integration" }' 'schema-next = { path = "vendor-sources/schema-next" }'

              substituteInPlace $out/vendor-sources/schema-rust-next/Cargo.toml \
                --replace-fail 'schema-next = { git = "https://github.com/LiGoldragon/schema-next.git", branch = "structural-forms-integration" }' 'schema-next = { path = "../schema-next" }' \
                --replace-fail 'nota-next = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "structural-forms-integration" }' 'nota-next = { path = "../nota-next" }' \
                --replace-fail 'signal-frame = { git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main" }' 'signal-frame = { path = "../signal-frame" }' \
                --replace-fail 'triad-runtime = { git = "https://github.com/LiGoldragon/triad-runtime.git", branch = "structural-forms-integration" }' 'triad-runtime = { path = "../triad-runtime" }'

              substituteInPlace $out/vendor-sources/schema-next/Cargo.toml \
                --replace-fail 'nota-next = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "main" }' 'nota-next = { path = "../nota-next" }'

              substituteInPlace $out/vendor-sources/schema-next/schema-cc/Cargo.toml \
                --replace-fail 'nota-next    = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "main" }' 'nota-next    = { path = "../../nota-next" }'

              substituteInPlace $out/vendor-sources/sema-engine/Cargo.toml \
                --replace-fail 'sema = { git = "https://github.com/LiGoldragon/sema.git", branch = "main" }' 'sema = { path = "../sema" }' \
                --replace-fail 'signal-frame = { git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main", default-features = false }' 'signal-frame = { path = "../signal-frame", default-features = false }' \
                --replace-fail 'signal-sema = { git = "https://github.com/LiGoldragon/signal-sema.git", branch = "main", default-features = false }' 'signal-sema = { path = "../signal-sema", default-features = false }'

              substituteInPlace $out/vendor-sources/sema-engine-previous/Cargo.toml \
                --replace-fail 'sema = { git = "https://github.com/LiGoldragon/sema.git", branch = "main" }' 'sema = { path = "../sema" }' \
                --replace-fail 'signal-frame = { git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main", default-features = false }' 'signal-frame = { path = "../signal-frame", default-features = false }' \
                --replace-fail 'signal-sema = { git = "https://github.com/LiGoldragon/signal-sema.git", branch = "main", default-features = false }' 'signal-sema = { path = "../signal-sema", default-features = false }'

              substituteInPlace $out/vendor-sources/sema-engine-layout3/Cargo.toml \
                --replace-fail 'sema = { git = "https://github.com/LiGoldragon/sema.git", branch = "main" }' 'sema = { path = "../sema" }' \
                --replace-fail 'signal-frame = { git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main", default-features = false }' 'signal-frame = { path = "../signal-frame", default-features = false }' \
                --replace-fail 'signal-sema = { git = "https://github.com/LiGoldragon/signal-sema.git", branch = "main", default-features = false }' 'signal-sema = { path = "../signal-sema", default-features = false }'

              substituteInPlace $out/vendor-sources/triad-runtime/Cargo.toml \
                --replace-fail 'signal-frame = { git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main" }' 'signal-frame = { path = "../signal-frame" }'

              substituteInPlace $out/vendor-sources/signal-frame/Cargo.toml \
                --replace-fail 'nota-next = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "main", optional = true }' 'nota-next = { path = "../nota-next", optional = true }' \
                --replace-fail 'nota-next = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "main" }' 'nota-next = { path = "../nota-next" }'

              substituteInPlace $out/vendor-sources/signal-sema/Cargo.toml \
                --replace-fail 'nota-next  = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "main", optional = true }' 'nota-next  = { path = "../nota-next", optional = true }' \
                --replace-fail 'nota-next  = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "main" }' 'nota-next  = { path = "../nota-next" }'

              substituteInPlace $out/vendor-sources/signal-spirit/Cargo.toml \
                --replace-fail '{ git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main", default-features = false }' '{ path = "../signal-frame", default-features = false }' \
                --replace-fail '{ git = "https://github.com/LiGoldragon/nota-next.git", branch = "structural-forms-integration", optional = true }' '{ path = "../nota-next", optional = true }' \
                --replace-fail '{ git = "https://github.com/LiGoldragon/version-projection.git", branch = "main", default-features = false }' '{ path = "../version-projection", default-features = false }' \
                --replace-fail '{ git = "https://github.com/LiGoldragon/schema-rust-next.git", branch = "structural-forms-integration" }' '{ path = "../schema-rust-next" }' \
                --replace-fail '{ git = "https://github.com/LiGoldragon/nota-next.git", branch = "structural-forms-integration" }' '{ path = "../nota-next" }'

              substituteInPlace $out/vendor-sources/meta-signal-spirit/Cargo.toml \
                --replace-fail '{ git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main", default-features = false }' '{ path = "../signal-frame", default-features = false }' \
                --replace-fail '{ git = "https://github.com/LiGoldragon/signal-spirit.git", branch = "structural-forms-integration", default-features = false }' '{ path = "../signal-spirit", default-features = false }' \
                --replace-fail '{ git = "https://github.com/LiGoldragon/nota-next.git", branch = "structural-forms-integration", optional = true }' '{ path = "../nota-next", optional = true }' \
                --replace-fail '{ git = "https://github.com/LiGoldragon/schema-rust-next.git", branch = "structural-forms-integration" }' '{ path = "../schema-rust-next" }' \
                --replace-fail '{ git = "https://github.com/LiGoldragon/nota-next.git", branch = "structural-forms-integration" }' '{ path = "../nota-next" }'

              substituteInPlace $out/vendor-sources/signal-agent/Cargo.toml \
                --replace-fail '{ git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main", default-features = false }' '{ path = "../signal-frame", default-features = false }' \
                --replace-fail '{ git = "https://github.com/LiGoldragon/nota-next.git", branch = "structural-forms-integration", optional = true }' '{ path = "../nota-next", optional = true }' \
                --replace-fail '{ git = "https://github.com/LiGoldragon/schema-rust-next.git", branch = "structural-forms-integration" }' '{ path = "../schema-rust-next" }'

              substituteInPlace $out/vendor-sources/meta-signal-agent/Cargo.toml \
                --replace-fail 'nota-next    = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "structural-forms-integration", optional = true }' 'nota-next    = { path = "../nota-next", optional = true }' \
                --replace-fail 'signal-frame = { git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main", default-features = false }' 'signal-frame = { path = "../signal-frame", default-features = false }' \
                --replace-fail 'schema-rust-next = { git = "https://github.com/LiGoldragon/schema-rust-next.git", branch = "structural-forms-integration" }' 'schema-rust-next = { path = "../schema-rust-next" }'

              substituteInPlace $out/vendor-sources/agent/Cargo.toml \
                --replace-fail 'nota-next        = { git = "https://github.com/LiGoldragon/nota-next.git", branch = "structural-forms-integration" }' 'nota-next        = { path = "../nota-next" }' \
                --replace-fail 'signal-frame     = { git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main" }' 'signal-frame     = { path = "../signal-frame" }' \
                --replace-fail 'signal-agent      = { git = "https://github.com/LiGoldragon/signal-agent.git", branch = "main" }' 'signal-agent      = { path = "../signal-agent" }' \
                --replace-fail 'meta-signal-agent = { git = "https://github.com/LiGoldragon/meta-signal-agent.git", branch = "main" }' 'meta-signal-agent = { path = "../meta-signal-agent" }' \
                --replace-fail 'triad-runtime    = { git = "https://github.com/LiGoldragon/triad-runtime.git", branch = "structural-forms-integration" }' 'triad-runtime    = { path = "../triad-runtime" }' \
                --replace-fail 'schema-rust-next = { git = "https://github.com/LiGoldragon/schema-rust-next.git", branch = "structural-forms-integration" }' 'schema-rust-next = { path = "../schema-rust-next" }'

              substituteInPlace $out/vendor-sources/version-projection/Cargo.toml \
                --replace-fail '{ git = "https://github.com/LiGoldragon/nota-next.git", branch = "main", optional = true }' '{ path = "../nota-next", optional = true }' \
                --replace-fail '{ git = "https://github.com/LiGoldragon/nota-next.git", branch = "main" }' '{ path = "../nota-next" }'

              cat >> $out/Cargo.toml <<'EOF'
              [patch."https://github.com/LiGoldragon/nota-next.git"]
              nota-next = { path = "vendor-sources/nota-next" }
              nota-next-derive = { path = "vendor-sources/nota-next/derive" }

              [patch."https://github.com/LiGoldragon/schema-next.git"]
              schema-next = { path = "vendor-sources/schema-next" }

              [patch."https://github.com/LiGoldragon/schema-rust-next.git"]
              schema-rust-next = { path = "vendor-sources/schema-rust-next" }

              [patch."https://github.com/LiGoldragon/sema.git"]
              sema = { path = "vendor-sources/sema" }

              [patch."https://github.com/LiGoldragon/sema-engine.git"]
              sema-engine = { path = "vendor-sources/sema-engine" }
              sema-engine-layout3 = { path = "vendor-sources/sema-engine-layout3", package = "sema-engine" }
              sema-engine-previous = { path = "vendor-sources/sema-engine-previous", package = "sema-engine" }

              [patch."https://github.com/LiGoldragon/signal-frame.git"]
              signal-frame = { path = "vendor-sources/signal-frame" }
              signal-frame-macros = { path = "vendor-sources/signal-frame/macros" }

              [patch."https://github.com/LiGoldragon/signal-sema.git"]
              signal-sema = { path = "vendor-sources/signal-sema" }

              [patch."https://github.com/LiGoldragon/triad-runtime.git"]
              triad-runtime = { path = "vendor-sources/triad-runtime" }

              [patch."https://github.com/LiGoldragon/signal-spirit.git"]
              signal-spirit = { path = "vendor-sources/signal-spirit" }

              [patch."https://github.com/LiGoldragon/meta-signal-spirit.git"]
              meta-signal-spirit = { path = "vendor-sources/meta-signal-spirit" }

              [patch."https://github.com/LiGoldragon/signal-agent.git"]
              signal-agent = { path = "vendor-sources/signal-agent" }

              [patch."https://github.com/LiGoldragon/meta-signal-agent.git"]
              meta-signal-agent = { path = "vendor-sources/meta-signal-agent" }

              [patch."https://github.com/LiGoldragon/agent.git"]
              agent = { path = "vendor-sources/agent" }

              [patch."https://github.com/LiGoldragon/version-projection.git"]
              version-projection = { path = "vendor-sources/version-projection" }
              EOF

            '';
        # The vendor step maps every LiGoldragon git source onto one local
        # path per repository, so the lock must end up with ONE entry per
        # (name, version). If transient branch and main entries for the same
        # package appear, source-stripping would make them collide ("specified
        # twice"). Dedup keeps the entry whose original source matches the
        # vendored reference for that repository.
        patchedCargoLock = pkgs.runCommand "spirit-patched-Cargo.lock" { } ''
          ${pkgs.python3}/bin/python3 - ${./Cargo.lock} "$out" <<'PYEOF'
          import re, sys

          preferred_reference = {
              "schema-next": "structural-forms-integration",
              "schema-rust-next": "structural-forms-integration",
              "triad-runtime": "structural-forms-integration",
          }
          preferred_version = {
              "nota-next": "0.5.0",
              "nota-next-derive": "0.3.0",
          }
          path_dependency_names = (
              "meta-signal-agent",
              "meta-signal-spirit",
              "nota-next",
              "nota-next-derive",
              "schema-next",
              "schema-rust-next",
              "agent",
              "sema",
              "signal-agent",
              "signal-frame",
              "signal-frame-macros",
              "signal-sema",
              "signal-spirit",
              "triad-runtime",
              "version-projection",
          )

          source_text = open(sys.argv[1]).read()
          blocks = source_text.split("[[package]]")
          header, entries = blocks[0], blocks[1:]

          def field(entry, name):
              found = re.search(r'^%s = "([^"]*)"' % name, entry, re.M)
              return found.group(1) if found else ""

          kept, seen = [], {}
          for entry in entries:
              name = field(entry, "name")
              version = field(entry, "version")
              preferred = preferred_version.get(name)
              if preferred and preferred != version:
                  continue
              key = (name, version)
              source = field(entry, "source")
              if key in seen:
                  wanted = preferred_reference.get(key[0])
                  if wanted and wanted in source:
                      kept[seen[key]] = entry
                  continue
              seen[key] = len(kept)
              kept.append(entry)

          stripped = []
          for entry in kept:
              entry = "\n".join(
                  line for line in entry.split("\n")
                  if not line.startswith('source = "git+https://github.com/LiGoldragon/')
              )
              entry = re.sub(
                  r' \((git\+https://github\.com/LiGoldragon/[^)]+)\)',
                  "",
                  entry,
              )
              for dependency_name in path_dependency_names:
                  entry = re.sub(
                      r'"' + re.escape(dependency_name) + r'(?: [^"]+)?",',
                      '"' + dependency_name + '",',
                      entry,
                  )
              stripped.append(entry)
          open(sys.argv[2], "w").write(header + "".join("[[package]]" + entry for entry in stripped))
          PYEOF
        '';
        cargoVendorDirectory = craneLib.vendorCargoDeps {
          inherit src;
          cargoLock = patchedCargoLock;
        };
        commonArguments = {
          inherit src cargoVendorDirectory;
          cargoLock = patchedCargoLock;
          strictDeps = true;
        };
        binaryCargoArtifacts = craneLib.buildDepsOnly (
          commonArguments
          // {
            cargoExtraArgs = "--no-default-features";
          }
        );
        notaTextCargoArtifacts = craneLib.buildDepsOnly (
          commonArguments
          // {
            cargoExtraArgs = "--features nota-text";
          }
        );
        agentGuardianCargoArtifacts = craneLib.buildDepsOnly (
          commonArguments
          // {
            cargoExtraArgs = "--features agent-guardian";
          }
        );
        testingTraceCargoArtifacts = craneLib.buildDepsOnly (
          commonArguments
          // {
            cargoExtraArgs = "--features testing-trace";
          }
        );
        notaTextTestingTraceCargoArtifacts = craneLib.buildDepsOnly (
          commonArguments
          // {
            cargoExtraArgs = "--features nota-text,testing-trace";
          }
        );
        daemonPackage = craneLib.buildPackage (
          commonArguments
          // {
            cargoArtifacts = agentGuardianCargoArtifacts;
            cargoExtraArgs = "--features agent-guardian --bin spirit-daemon";
          }
        );
        cliPackage = craneLib.buildPackage (
          commonArguments
          // {
            cargoArtifacts = notaTextCargoArtifacts;
            cargoExtraArgs = "--features nota-text --bin spirit";
          }
        );
        metaSpiritCliPackage = craneLib.buildPackage (
          commonArguments
          // {
            cargoArtifacts = notaTextCargoArtifacts;
            cargoExtraArgs = "--features nota-text --bin meta-spirit";
          }
        );
        configurationWriterPackage = craneLib.buildPackage (
          commonArguments
          // {
            cargoArtifacts = notaTextCargoArtifacts;
            cargoExtraArgs = "--features nota-text --bin spirit-write-configuration";
          }
        );
        storeMigrationPackage = craneLib.buildPackage (
          commonArguments
          // {
            cargoArtifacts = notaTextCargoArtifacts;
            cargoExtraArgs = "--features production-migration --bin spirit-migrate-store";
          }
        );
        traceDaemonPackage = craneLib.buildPackage (
          commonArguments
          // {
            cargoArtifacts = testingTraceCargoArtifacts;
            cargoExtraArgs = "--features testing-trace --bin spirit-daemon";
          }
        );
        traceCliPackage = craneLib.buildPackage (
          commonArguments
          // {
            cargoArtifacts = notaTextTestingTraceCargoArtifacts;
            cargoExtraArgs = "--features nota-text,testing-trace --bin spirit";
          }
        );
        combinedPackage = pkgs.runCommand "spirit" { } ''
          mkdir -p "$out/bin"
          ln -s "${cliPackage}/bin/spirit" "$out/bin/spirit"
          ln -s "${metaSpiritCliPackage}/bin/meta-spirit" "$out/bin/meta-spirit"
          ln -s "${daemonPackage}/bin/spirit-daemon" "$out/bin/spirit-daemon"
          ln -s "${configurationWriterPackage}/bin/spirit-write-configuration" "$out/bin/spirit-write-configuration"
          ln -s "${storeMigrationPackage}/bin/spirit-migrate-store" "$out/bin/spirit-migrate-store"
        '';
        traceCombinedPackage = pkgs.runCommand "spirit-trace" { } ''
          mkdir -p "$out/bin"
          ln -s "${traceCliPackage}/bin/spirit" "$out/bin/spirit"
          ln -s "${traceDaemonPackage}/bin/spirit-daemon" "$out/bin/spirit-daemon"
          ln -s "${configurationWriterPackage}/bin/spirit-write-configuration" "$out/bin/spirit-write-configuration"
        '';
        nixIntegrationRunner = pkgs.writeShellApplication {
          name = "spirit-nix-integration-tests";
          runtimeInputs = [
            pkgs.nix
            toolchain
          ];
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
        packages.configuration-writer = configurationWriterPackage;
        packages.store-migration = storeMigrationPackage;
        packages.trace = traceCombinedPackage;
        packages."trace-cli" = traceCliPackage;
        packages."trace-daemon" = traceDaemonPackage;
        apps.nix-integration-tests = {
          type = "app";
          program = "${nixIntegrationRunner}/bin/spirit-nix-integration-tests";
          meta.description = "Run Nix-built spirit integration tests";
        };
        checks = {
          build = craneLib.cargoBuild (
            commonArguments
            // {
              cargoArtifacts = binaryCargoArtifacts;
              cargoExtraArgs = "--no-default-features";
            }
          );
          build-nota-text = craneLib.cargoBuild (
            commonArguments
            // {
              cargoArtifacts = notaTextCargoArtifacts;
              cargoExtraArgs = "--features nota-text";
            }
          );
          test = craneLib.cargoTest (
            commonArguments
            // {
              cargoArtifacts = binaryCargoArtifacts;
              cargoExtraArgs = "--no-default-features";
            }
          );
          test-nota-text = craneLib.cargoTest (
            commonArguments
            // {
              cargoArtifacts = notaTextCargoArtifacts;
              cargoExtraArgs = "--features nota-text";
            }
          );
          test-configuration-writer-process-boundary = craneLib.cargoTest (
            commonArguments
            // {
              cargoArtifacts = notaTextCargoArtifacts;
              cargoExtraArgs = "--features nota-text --test process_boundary configuration_writer_prebuilds_binary_archive_for_daemon_startup -- --exact";
            }
          );
          test-testing-trace = craneLib.cargoTest (
            commonArguments
            // {
              cargoArtifacts = testingTraceCargoArtifacts;
              cargoExtraArgs = "--features testing-trace --test instrumentation_logging";
            }
          );
          test-testing-trace-process-boundary = craneLib.cargoTest (
            commonArguments
            // {
              cargoArtifacts = notaTextTestingTraceCargoArtifacts;
              cargoExtraArgs = "--features nota-text,testing-trace --test process_boundary cli_receives_testing_trace_events_from_daemon_trace_socket -- --exact";
            }
          );
          no-old-signal-macro = pkgs.runCommand "spirit-no-old-signal-macro" { } ''
            if grep -R "signal_channel!" ${src}/build.rs ${src}/schema ${src}/src ${src}/tests; then
              echo "spirit must not use the old signal_channel macro" >&2
              exit 1
            fi
            touch $out
          '';
          generated-schema-source-checked-in =
            pkgs.runCommand "spirit-generated-schema-source-checked-in" { }
              ''
                # Positive freshness proof runs through the cargo build/test
                # checks: build.rs decodes each plane schema as SchemaSource,
                # round-trips canonical source and rkyv archive values, emits
                # Rust from the typed schema-in-Rust values, and compares the
                # checked-in generated Rust. This check only keeps retired
                # side-channel source paths absent.
                test ! -e ${src}/schema/signal.schema
                test ! -e ${src}/schema/domain.schema
                test ! -e ${src}/src/schema/signal.rs
                test ! -e ${src}/src/schema/domain.rs
                test -f ${src}/src/schema/nexus.rs
                test -f ${src}/src/schema/sema.rs
                test -f ${src}/src/schema/daemon.rs
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
          operator-271-closed-claims = craneLib.cargoTest (
            commonArguments
            // {
              cargoArtifacts = binaryCargoArtifacts;
              cargoExtraArgs = "--no-default-features --test operator_271_closed_claims";
            }
          );
          fmt = craneLib.cargoFmt { inherit src; };
          clippy = craneLib.cargoClippy (
            commonArguments
            // {
              cargoArtifacts = binaryCargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- -D warnings";
            }
          );
          clippy-nota-text = craneLib.cargoClippy (
            commonArguments
            // {
              cargoArtifacts = notaTextCargoArtifacts;
              cargoClippyExtraArgs = "--features nota-text --all-targets -- -D warnings";
            }
          );
          clippy-testing-trace = craneLib.cargoClippy (
            commonArguments
            // {
              cargoArtifacts = notaTextTestingTraceCargoArtifacts;
              cargoClippyExtraArgs = "--features nota-text,testing-trace --all-targets -- -D warnings";
            }
          );
          doc = craneLib.cargoDoc (
            commonArguments
            // {
              cargoArtifacts = binaryCargoArtifacts;
              RUSTDOCFLAGS = "-D warnings";
            }
          );
        };
        devShells.default = pkgs.mkShell {
          name = "spirit";
          packages = [
            pkgs.jujutsu
            pkgs.pkg-config
            toolchain
          ];
        };
      }
    );
}
