{
  description = "spirit — runnable schema-derived Spirit pilot";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-build = {
      url = "github:LiGoldragon/rust-build";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    kameo-source = {
      url = "github:LiGoldragon/kameo";
      flake = false;
    };
    nota-source = {
      url = "github:LiGoldragon/nota";
      flake = false;
    };
    schema-source = {
      url = "github:LiGoldragon/schema";
      flake = false;
    };
    schema-language-source = {
      url = "github:LiGoldragon/schema-language/6aae825d668e3f607a2754afa6b7d94e9f246c41";
      flake = false;
    };
    schema-rust-source = {
      url = "github:LiGoldragon/schema-rust";
      flake = false;
    };
    sema-source = {
      url = "github:LiGoldragon/sema";
      flake = false;
    };
    sema-engine-source = {
      url = "github:LiGoldragon/sema-engine";
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
      url = "git+https://github.com/LiGoldragon/signal-frame.git?ref=main";
      flake = false;
    };
    signal-sema-source = {
      url = "github:LiGoldragon/signal-sema";
      flake = false;
    };
    triad-runtime-source = {
      url = "git+https://github.com/LiGoldragon/triad-runtime.git?ref=main";
      flake = false;
    };
    criome-source = {
      url = "git+https://github.com/LiGoldragon/criome.git?ref=main";
      flake = false;
    };
    signal-criome-source = {
      url = "git+https://github.com/LiGoldragon/signal-criome.git?ref=main";
      flake = false;
    };
    meta-signal-criome-source = {
      url = "git+https://github.com/LiGoldragon/meta-signal-criome.git?ref=main";
      flake = false;
    };
    signal-spirit-source = {
      url = "github:LiGoldragon/signal-spirit";
      flake = false;
    };
    nota-text-query-source = {
      url = "github:LiGoldragon/nota-text-query";
      flake = false;
    };
    meta-signal-spirit-source = {
      url = "github:LiGoldragon/meta-signal-spirit";
      flake = false;
    };
    signal-agent-source = {
      url = "github:LiGoldragon/signal-agent";
      flake = false;
    };
    signal-introspect-source = {
      url = "github:LiGoldragon/signal-introspect";
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
    mirror-source = {
      url = "github:LiGoldragon/mirror";
      flake = false;
    };
    meta-signal-mirror-source = {
      url = "github:LiGoldragon/meta-signal-mirror";
      flake = false;
    };
    signal-mirror-source = {
      url = "github:LiGoldragon/signal-mirror";
      flake = false;
    };
    router-source = {
      url = "github:LiGoldragon/router";
      flake = false;
    };
    meta-signal-router-source = {
      url = "github:LiGoldragon/meta-signal-router";
      flake = false;
    };
    signal-router-source = {
      url = "github:LiGoldragon/signal-router";
      flake = false;
    };
    signal-standard-source = {
      url = "github:LiGoldragon/signal-standard";
      flake = false;
    };
    signal-message-source = {
      url = "github:LiGoldragon/signal-message";
      flake = false;
    };
    signal-harness-source = {
      url = "github:LiGoldragon/signal-harness";
      flake = false;
    };
    signal-persona-source = {
      url = "github:LiGoldragon/signal-persona";
      flake = false;
    };
    signal-mind-source = {
      url = "github:LiGoldragon/signal-mind";
      flake = false;
    };
    signal-domain-source = {
      url = "git+https://github.com/LiGoldragon/signal-domain.git?ref=main";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-build,
      kameo-source,
      nota-source,
      schema-source,
      schema-language-source,
      schema-rust-source,
      sema-source,
      sema-engine-source,
      sema-engine-previous-source,
      sema-engine-layout3-source,
      signal-frame-source,
      signal-sema-source,
      triad-runtime-source,
      criome-source,
      signal-criome-source,
      meta-signal-criome-source,
      signal-spirit-source,
      nota-text-query-source,
      meta-signal-spirit-source,
      signal-agent-source,
      signal-introspect-source,
      meta-signal-agent-source,
      agent-source,
      version-projection-source,
      mirror-source,
      meta-signal-mirror-source,
      signal-mirror-source,
      router-source,
      meta-signal-router-source,
      signal-router-source,
      signal-standard-source,
      signal-message-source,
      signal-harness-source,
      signal-persona-source,
      signal-mind-source,
      signal-domain-source,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        rust = rust-build.lib.${system}.fromPkgs pkgs;
        inherit (rust) craneLib toolchain;
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
        cleanSource = rust.cleanSource {
          root = ./.;
          extraFilters = [
            schemaFilter
            scriptFilter
            promptFilter
          ];
        };
        src =
          pkgs.runCommand "spirit-source-with-local-schema-patches"
            {
              kameoSource = kameo-source;
              notaNextSource = nota-source;
              schemaNextSource = schema-source;
              schemaLanguageSource = schema-language-source;
              schemaRustNextSource = schema-rust-source;
              semaSource = sema-source;
              semaEngineSource = sema-engine-source;
              semaEnginePreviousSource = sema-engine-previous-source;
              semaEngineLayout3Source = sema-engine-layout3-source;
              signalFrameSource = signal-frame-source;
              signalSemaSource = signal-sema-source;
              triadRuntimeSource = triad-runtime-source;
              criomeSource = criome-source;
              signalCriomeSource = signal-criome-source;
              metaSignalCriomeSource = meta-signal-criome-source;
              signalSpiritSource = signal-spirit-source;
              notaTextQuerySource = nota-text-query-source;
              metaSignalSpiritSource = meta-signal-spirit-source;
              signalAgentSource = signal-agent-source;
              signalIntrospectSource = signal-introspect-source;
              metaSignalAgentSource = meta-signal-agent-source;
              agentSource = agent-source;
              versionProjectionSource = version-projection-source;
              mirrorSource = mirror-source;
              metaSignalMirrorSource = meta-signal-mirror-source;
              signalMirrorSource = signal-mirror-source;
              routerSource = router-source;
              metaSignalRouterSource = meta-signal-router-source;
              signalRouterSource = signal-router-source;
              signalStandardSource = signal-standard-source;
              signalMessageSource = signal-message-source;
              signalHarnessSource = signal-harness-source;
              signalPersonaSource = signal-persona-source;
              signalMindSource = signal-mind-source;
              signalDomainSource = signal-domain-source;
            }
            ''
              cp -R ${cleanSource} $out
              chmod -R u+w $out
              mkdir -p $out/vendor-sources
              cp -R "$kameoSource" $out/vendor-sources/kameo
              cp -R "$notaNextSource" $out/vendor-sources/nota
              cp -R "$schemaNextSource" $out/vendor-sources/schema
              cp -R "$schemaLanguageSource" $out/vendor-sources/schema-language
              cp -R "$schemaRustNextSource" $out/vendor-sources/schema-rust
              cp -R "$semaSource" $out/vendor-sources/sema
              cp -R "$semaEngineSource" $out/vendor-sources/sema-engine
              cp -R "$semaEnginePreviousSource" $out/vendor-sources/sema-engine-previous
              cp -R "$semaEngineLayout3Source" $out/vendor-sources/sema-engine-layout3
              cp -R "$signalFrameSource" $out/vendor-sources/signal-frame
              cp -R "$signalSemaSource" $out/vendor-sources/signal-sema
              cp -R "$triadRuntimeSource" $out/vendor-sources/triad-runtime
              cp -R "$criomeSource" $out/vendor-sources/criome
              cp -R "$signalCriomeSource" $out/vendor-sources/signal-criome
              cp -R "$metaSignalCriomeSource" $out/vendor-sources/meta-signal-criome
              cp -R "$signalSpiritSource" $out/vendor-sources/signal-spirit
              cp -R "$notaTextQuerySource" $out/vendor-sources/nota-text-query
              cp -R "$metaSignalSpiritSource" $out/vendor-sources/meta-signal-spirit
              cp -R "$signalAgentSource" $out/vendor-sources/signal-agent
              cp -R "$signalIntrospectSource" $out/vendor-sources/signal-introspect
              cp -R "$metaSignalAgentSource" $out/vendor-sources/meta-signal-agent
              cp -R "$agentSource" $out/vendor-sources/agent
              cp -R "$versionProjectionSource" $out/vendor-sources/version-projection
              cp -R "$mirrorSource" $out/vendor-sources/mirror
              cp -R "$metaSignalMirrorSource" $out/vendor-sources/meta-signal-mirror
              cp -R "$signalMirrorSource" $out/vendor-sources/signal-mirror
              cp -R "$routerSource" $out/vendor-sources/router
              cp -R "$metaSignalRouterSource" $out/vendor-sources/meta-signal-router
              cp -R "$signalRouterSource" $out/vendor-sources/signal-router
              cp -R "$signalStandardSource" $out/vendor-sources/signal-standard
              cp -R "$signalMessageSource" $out/vendor-sources/signal-message
              cp -R "$signalHarnessSource" $out/vendor-sources/signal-harness
              cp -R "$signalPersonaSource" $out/vendor-sources/signal-persona
              cp -R "$signalMindSource" $out/vendor-sources/signal-mind
              cp -R "$signalDomainSource" $out/vendor-sources/signal-domain
              chmod -R u+w $out/vendor-sources

              ${pkgs.python3}/bin/python3 - "$out/vendor-sources" <<'PYEOF'
              from pathlib import Path
              import sys

              vendor_sources = Path(sys.argv[1])
              branch_aliases = (
                  ('branch = "structural-forms-integration"', 'branch = "main"'),
                  ('branch = "versioned-family-identity"', 'branch = "main"'),
              )
              for cargo_toml in vendor_sources.rglob("Cargo.toml"):
                  text = cargo_toml.read_text()
                  for original, replacement in branch_aliases:
                      text = text.replace(original, replacement)
                  cargo_toml.write_text(text)
              PYEOF

              substituteInPlace $out/Cargo.toml \
                --replace-fail 'nota = { package = "nota", git = "https://github.com/LiGoldragon/nota.git", branch = "main", optional = true }' 'nota = { path = "vendor-sources/nota", optional = true }' \
                --replace-fail 'nota = { package = "nota", git = "https://github.com/LiGoldragon/nota.git", branch = "main" }' 'nota = { path = "vendor-sources/nota" }' \
                --replace-fail 'nota-derive = { package = "nota-derive", git = "https://github.com/LiGoldragon/nota.git", branch = "main" }' 'nota-derive = { path = "vendor-sources/nota/derive" }' \
                --replace-fail 'sema-engine = { git = "https://github.com/LiGoldragon/sema-engine.git", branch = "main" }' 'sema-engine = { path = "vendor-sources/sema-engine" }' \
                --replace-fail 'sema-engine-previous = { git = "https://github.com/LiGoldragon/sema-engine.git", rev = "ebee6e44ba6ee4afcb26998007bcfd128641b54c", package = "sema-engine", optional = true }' 'sema-engine-previous = { path = "vendor-sources/sema-engine-previous", package = "sema-engine", optional = true }' \
                --replace-fail 'sema-engine-layout3 = { git = "https://github.com/LiGoldragon/sema-engine.git", rev = "dbe29427d9a2c6c194909385485ad42b008048b8", package = "sema-engine", optional = true }' 'sema-engine-layout3 = { path = "vendor-sources/sema-engine-layout3", package = "sema-engine", optional = true }' \
                --replace-fail 'signal-frame = { git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main" }' 'signal-frame = { path = "vendor-sources/signal-frame" }' \
                --replace-fail 'criome = { git = "https://github.com/LiGoldragon/criome.git", branch = "main", optional = true }' 'criome = { path = "vendor-sources/criome", optional = true }' \
                --replace-fail 'signal-criome = { git = "https://github.com/LiGoldragon/signal-criome.git", branch = "main", default-features = false, optional = true }' 'signal-criome = { path = "vendor-sources/signal-criome", default-features = false, optional = true }' \
                --replace-fail 'signal-router = { git = "https://github.com/LiGoldragon/signal-router.git", branch = "main", default-features = false, optional = true }' 'signal-router = { path = "vendor-sources/signal-router", default-features = false, optional = true }' \
                --replace-fail 'meta-signal-router = { git = "https://github.com/LiGoldragon/meta-signal-router.git", branch = "main", default-features = false, optional = true }' 'meta-signal-router = { path = "vendor-sources/meta-signal-router", default-features = false, optional = true }' \
                --replace-fail 'meta-signal-criome = { git = "https://github.com/LiGoldragon/meta-signal-criome.git", branch = "main", default-features = false, optional = true }' 'meta-signal-criome = { path = "vendor-sources/meta-signal-criome", default-features = false, optional = true }' \
                --replace-fail 'signal-agent = { git = "https://github.com/LiGoldragon/signal-agent.git", branch = "main", optional = true }' 'signal-agent = { path = "vendor-sources/signal-agent", optional = true }' \
                --replace-fail 'signal-introspect = { git = "https://github.com/LiGoldragon/signal-introspect.git", branch = "main", default-features = false, optional = true }' 'signal-introspect = { path = "vendor-sources/signal-introspect", default-features = false, optional = true }' \
                --replace-fail 'signal-persona = { git = "https://github.com/LiGoldragon/signal-persona.git", branch = "main", optional = true }' 'signal-persona = { path = "vendor-sources/signal-persona", optional = true }' \
                --replace-fail 'signal-spirit = { git = "https://github.com/LiGoldragon/signal-spirit.git", rev = "1cf7c010029de46369b742687da4fa1ca6def9a9" }' 'signal-spirit = { path = "vendor-sources/signal-spirit" }' \
                --replace-fail 'meta-signal-spirit = { git = "https://github.com/LiGoldragon/meta-signal-spirit.git", rev = "0a7a2438c8e5d57cb1fd413452d0a7ddad4fb9b3" }' 'meta-signal-spirit = { path = "vendor-sources/meta-signal-spirit" }' \
                --replace-fail 'nota-text-query = { git = "https://github.com/LiGoldragon/nota-text-query.git", rev = "6140a3e9afe3f81c18a39cb0a11dec4eab68b561", default-features = false }' 'nota-text-query = { path = "vendor-sources/nota-text-query", default-features = false }' \
                --replace-fail 'triad-runtime = { git = "https://github.com/LiGoldragon/triad-runtime.git", branch = "main" }' 'triad-runtime = { path = "vendor-sources/triad-runtime" }' \
                --replace-fail 'schema-rust = { package = "schema-rust", git = "https://github.com/LiGoldragon/schema-rust.git", rev = "f3b4563163dd11ba1cbbcca8081701ab7830b8f5" }' 'schema-rust = { path = "vendor-sources/schema-rust", package = "schema-rust" }' \
                --replace-fail 'agent = { git = "https://github.com/LiGoldragon/agent.git", branch = "main", features = ["live-provider"] }' 'agent = { path = "vendor-sources/agent", features = ["live-provider"] }' \
                --replace-fail 'schema = { package = "schema", git = "https://github.com/LiGoldragon/schema.git", rev = "92bed64ad644ce4caa2827398ada5b7b79221165" }' 'schema = { path = "vendor-sources/schema" }' \
                --replace-fail 'schema-cc = { package = "schema-cc", git = "https://github.com/LiGoldragon/schema.git", rev = "92bed64ad644ce4caa2827398ada5b7b79221165" }' 'schema-cc = { path = "vendor-sources/schema/schema-cc" }' \
                --replace-fail 'schema-language = { git = "https://github.com/LiGoldragon/schema-language.git", rev = "6aae825d668e3f607a2754afa6b7d94e9f246c41" }' 'schema-language = { path = "vendor-sources/schema-language" }' \
                --replace-fail 'signal-sema = { git = "https://github.com/LiGoldragon/signal-sema.git", branch = "main" }' 'signal-sema = { path = "vendor-sources/signal-sema" }'

              ${pkgs.python3}/bin/python3 - "$out/vendor-sources/schema-rust/Cargo.toml" <<'PYEOF'
              from pathlib import Path
              import sys

              cargo_toml = Path(sys.argv[1])
              text = cargo_toml.read_text()
              replacements = {
                  'schema = { git = "https://github.com/LiGoldragon/schema.git", branch = "main" }': 'schema = { path = "../schema" }',
                  'schema = { git = "https://github.com/LiGoldragon/schema.git", branch = "structural-forms-integration" }': 'schema = { path = "../schema" }',
                  'schema = { package = "schema", git = "https://github.com/LiGoldragon/schema.git", branch = "main" }': 'schema = { path = "../schema" }',
                  'nota = { git = "https://github.com/LiGoldragon/nota.git", branch = "main" }': 'nota = { path = "../nota" }',
                  'nota = { git = "https://github.com/LiGoldragon/nota.git", branch = "structural-forms-integration" }': 'nota = { path = "../nota" }',
                  'nota = { package = "nota", git = "https://github.com/LiGoldragon/nota.git", branch = "main" }': 'nota = { path = "../nota" }',
                  'sema-engine = { git = "https://github.com/LiGoldragon/sema-engine.git", branch = "versioned-family-identity" }': 'sema-engine = { path = "../sema-engine" }',
                  'signal-frame = { git = "https://github.com/LiGoldragon/signal-frame.git", branch = "main" }': 'signal-frame = { path = "../signal-frame" }',
                  'triad-runtime = { git = "https://github.com/LiGoldragon/triad-runtime.git", branch = "main" }': 'triad-runtime = { path = "../triad-runtime" }',
                  'triad-runtime = { git = "https://github.com/LiGoldragon/triad-runtime.git", branch = "structural-forms-integration" }': 'triad-runtime = { path = "../triad-runtime" }',
              }
              for original, replacement in replacements.items():
                  text = text.replace(original, replacement)

              cargo_toml.write_text(text)
              PYEOF

              ${pkgs.python3}/bin/python3 - "$out/vendor-sources" <<'PYEOF'
              from pathlib import Path
              import os
              import re
              import sys

              vendor_sources = Path(sys.argv[1])
              repository_names = {
                  path.name
                  for path in vendor_sources.iterdir()
                  if path.is_dir() and (path / "Cargo.toml").exists()
              }
              repository_aliases = {
                  "nota": "nota",
                  "schema": "schema",
                  "schema-rust": "schema-rust",
              }
              legacy_suffix = "-" + "next"
              for producer in tuple(repository_aliases):
                  repository_aliases[producer + legacy_suffix] = repository_aliases[producer]

              def replacement_path(cargo_toml: Path, repository: str) -> str:
                  target = vendor_sources / repository
                  return Path(os.path.relpath(target, cargo_toml.parent)).as_posix()

              for cargo_toml in vendor_sources.rglob("Cargo.toml"):
                  text = cargo_toml.read_text()
                  git_repositories = set(repository_names) | set(repository_aliases)
                  for git_repository in sorted(git_repositories, key=len, reverse=True):
                      vendor_repository = repository_aliases.get(git_repository, git_repository)
                      if vendor_repository not in repository_names:
                          continue
                      escaped = re.escape(git_repository)
                      relative = replacement_path(cargo_toml, vendor_repository)
                      text = re.sub(
                          rf'git = "https://github\.com/LiGoldragon/{escaped}\.git", branch = "[^"]+"',
                          f'path = "{relative}"',
                          text,
                      )
                      text = re.sub(
                          rf'git = "https://github\.com/LiGoldragon/{escaped}\.git", rev = "[^"]+"',
                          f'path = "{relative}"',
                          text,
                      )
                  cargo_toml.write_text(text)
              PYEOF

              cat >> $out/Cargo.toml <<'EOF'
              [patch."https://github.com/LiGoldragon/nota.git"]
              nota = { path = "vendor-sources/nota" }
              nota-derive = { path = "vendor-sources/nota/derive" }

              [patch."https://github.com/LiGoldragon/kameo.git"]
              kameo = { path = "vendor-sources/kameo" }
              kameo_macros = { path = "vendor-sources/kameo/macros" }

              [patch."https://github.com/LiGoldragon/schema.git"]
              schema = { path = "vendor-sources/schema" }

              [patch."https://github.com/LiGoldragon/schema-rust.git"]
              schema-rust = { path = "vendor-sources/schema-rust" }

              [patch."https://github.com/LiGoldragon/schema-language.git"]
              schema-language = { path = "vendor-sources/schema-language" }
              schema-language-cc = { path = "vendor-sources/schema-language/schema-language-cc" }

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

              [patch."https://github.com/LiGoldragon/nota-text-query.git"]
              nota-text-query = { path = "vendor-sources/nota-text-query" }

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

              [patch."https://github.com/LiGoldragon/mirror.git"]
              mirror = { path = "vendor-sources/mirror" }

              [patch."https://github.com/LiGoldragon/meta-signal-mirror.git"]
              meta-signal-mirror = { path = "vendor-sources/meta-signal-mirror" }

              [patch."https://github.com/LiGoldragon/signal-mirror.git"]
              signal-mirror = { path = "vendor-sources/signal-mirror" }

              [patch."https://github.com/LiGoldragon/router.git"]
              router = { path = "vendor-sources/router" }

              [patch."https://github.com/LiGoldragon/criome.git"]
              criome = { path = "vendor-sources/criome" }

              [patch."https://github.com/LiGoldragon/signal-criome.git"]
              signal-criome = { path = "vendor-sources/signal-criome" }

              [patch."https://github.com/LiGoldragon/meta-signal-criome.git"]
              meta-signal-criome = { path = "vendor-sources/meta-signal-criome" }

              [patch."https://github.com/LiGoldragon/signal-introspect.git"]
              signal-introspect = { path = "vendor-sources/signal-introspect" }

              [patch."https://github.com/LiGoldragon/meta-signal-router.git"]
              meta-signal-router = { path = "vendor-sources/meta-signal-router" }

              [patch."https://github.com/LiGoldragon/signal-router.git"]
              signal-router = { path = "vendor-sources/signal-router" }

              [patch."https://github.com/LiGoldragon/signal-standard.git"]
              signal-standard = { path = "vendor-sources/signal-standard" }

              [patch."https://github.com/LiGoldragon/signal-message.git"]
              signal-message = { path = "vendor-sources/signal-message" }

              [patch."https://github.com/LiGoldragon/signal-harness.git"]
              signal-harness = { path = "vendor-sources/signal-harness" }

              [patch."https://github.com/LiGoldragon/signal-persona.git"]
              signal-persona = { path = "vendor-sources/signal-persona" }

              [patch."https://github.com/LiGoldragon/signal-mind.git"]
              signal-mind = { path = "vendor-sources/signal-mind" }

              [patch."https://github.com/LiGoldragon/signal-domain.git"]
              signal-domain = { path = "vendor-sources/signal-domain" }
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
              "schema": "main",
              "schema-rust": "main",
              "triad-runtime": "main",
          }
          preferred_version = {
              "nota": "0.5.1",
              "nota-derive": "0.3.0",
          }
          path_dependency_names = (
              "kameo",
              "kameo_macros",
              "meta-signal-agent",
              "meta-signal-spirit",
              "nota",
              "nota-derive",
              "schema",
              "schema-rust",
              "schema-language",
              "schema-language-cc",
              "agent",
              "sema",
              "signal-agent",
              "signal-frame",
              "signal-frame-macros",
              "signal-sema",
              "signal-spirit",
              "nota-text-query",
              "triad-runtime",
              "version-projection",
              "mirror",
              "meta-signal-mirror",
              "signal-mirror",
              "router",
              "criome",
              "signal-criome",
              "meta-signal-criome",
              "signal-introspect",
              "meta-signal-router",
              "signal-router",
              "signal-standard",
              "signal-message",
              "signal-harness",
              "signal-persona",
              "signal-mind",
              "signal-domain",
          )

          source_text = open(sys.argv[1]).read()
          blocks = source_text.split("[[package]]")
          header, entries = blocks[0], blocks[1:]

          def field(entry, name):
              found = re.search(r'^%s = "([^"]*)"' % name, entry, re.M)
              return found.group(1) if found else ""

          def dedup_key(entry):
              name = field(entry, "name")
              version = field(entry, "version")
              source = field(entry, "source")
              return (name, version)

          kept, seen = [], {}
          for entry in entries:
              name = field(entry, "name")
              version = field(entry, "version")
              preferred = preferred_version.get(name)
              if preferred and preferred != version:
                  continue
              key = dedup_key(entry)
              source = field(entry, "source")
              if key in seen:
                  wanted = preferred_reference.get(name)
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
        clusterAuthorizationCargoArtifacts = craneLib.buildDepsOnly (
          commonArguments
          // {
            cargoExtraArgs = "--features cluster-authorization-e2e";
          }
        );
        # THE EVERYWHERE-GATE LOOPCHECK (§3.9):
        # spirit-cluster-gates-acceptance-over-router-test.
        #
        # STATEFUL, not a pure check — the same posture as the founding proof
        # (`router-two-hosts-found-root-over-router-test`): the harness spawns
        # two real OS-threaded criome daemons, two real router runtimes on
        # loopback TCP, real Unix working/meta sockets under a process-local
        # temp directory, an in-process mirror service, and a real spirit
        # engine, synchronizing with wall-clock waits. So it is a named,
        # explicitly-run output
        # (`nix build .#spirit-cluster-gates-acceptance-over-router-test`),
        # not a `checks.*` entry auto-swept by `nix flake check`. It witnesses
        # disabled-era residue covered by one grant, acceptance gated on the
        # cluster grant, the fail-closed refused advance (head did NOT
        # advance), and the dead-round supersession retry.
        clusterAuthorizationLoopcheck = craneLib.cargoTest (
          commonArguments
          // {
            cargoArtifacts = clusterAuthorizationCargoArtifacts;
            cargoExtraArgs = "--features cluster-authorization-e2e";
            cargoTestExtraArgs = "--test cluster_authorization_over_router spirit_cluster_gates_acceptance_over_the_router -- --exact";
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
        renderPackage = craneLib.buildPackage (
          commonArguments
          // {
            cargoArtifacts = notaTextCargoArtifacts;
            cargoExtraArgs = "--features nota-text --bin spirit-render";
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
          ln -s "${renderPackage}/bin/spirit-render" "$out/bin/spirit-render"
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
        packages.render = renderPackage;
        packages.store-migration = storeMigrationPackage;
        packages.trace = traceCombinedPackage;
        packages."trace-cli" = traceCliPackage;
        packages."trace-daemon" = traceDaemonPackage;
        packages."spirit-cluster-gates-acceptance-over-router-test" =
          clusterAuthorizationLoopcheck;
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
          # The owner-only meta ObserveHeadObject surfaces the head entry's rkyv
          # body as hex; decoding it and reconstructing through
          # VersionedCommitLogEntry::new reproduces the head ObserveHead returns —
          # the REAL record body the two-VM criome-auth witness forwards and lands.
          spirit-observe-head-object-rehashes-to-head = craneLib.cargoTest (
            commonArguments
            // {
              cargoArtifacts = notaTextCargoArtifacts;
              cargoExtraArgs = "--features nota-text --test observe_head_object";
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
            ! grep -R "nota" ${src}/src/config.rs ${src}/src/daemon.rs ${src}/src/bin/spirit-daemon.rs
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
