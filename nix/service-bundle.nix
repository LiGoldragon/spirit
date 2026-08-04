{
  pkgs,
  lib,
  combinedPackage,
  judgePackage,
  judgeConfigPackage,
  judgeProviderPackage,
}:
{ stateDirectory }:
let
  isDotosAtom =
    value:
    builtins.isString value
    && value != ""
    && builtins.all (forbidden: !(lib.hasInfix forbidden value)) [
      " "
      "\t"
      "\n"
      "("
      ")"
      "["
      "]"
      "{"
      "}"
    ];
  stateDirectoryIsAbsolute = builtins.isString stateDirectory && lib.hasPrefix "/" stateDirectory;
  stateDirectoryIsDotosAtom = isDotosAtom stateDirectory;
  judgeModel = "gpt-5.6-luna";
  judgeReasoningEffort = "XHigh";
  judgeTimeoutMilliseconds = 180000;
  judgeSessionReference = "codex-login";

  socketPath = "${stateDirectory}/spirit.sock";
  metaSocketPath = "${stateDirectory}/meta-spirit.sock";
  judgeSocketPath = "${stateDirectory}/spirit-judge.sock";
  databasePath = "${stateDirectory}/spirit.sema";
  configurationPath = "spirit.config.rkyv";

  guardianJudgeConfiguration = "(Some (${judgeSocketPath} None None ${toString judgeTimeoutMilliseconds} None))";
  daemonConfiguration = pkgs.runCommand "spirit-daemon-configuration" { } ''
    set -eu

    mkdir -p "$out"
    ${combinedPackage}/bin/spirit-write-configuration \
      "(ConfigurationWriteRequest (${socketPath} (Some ${metaSocketPath}) ${databasePath} None Gating ${guardianJudgeConfiguration} $out/${configurationPath}))" \
      > "$out/configuration-written.dotos"
    test -s "$out/${configurationPath}"
  '';

  activateState = pkgs.writeShellScript "spirit-activation-state" ''
    set -eu

    state_directory=${lib.escapeShellArg stateDirectory}
    ${pkgs.coreutils}/bin/mkdir -p "$state_directory"
  '';

  initializeState = pkgs.writeShellScript "spirit-startup-state" ''
    set -eu

    state_directory=${lib.escapeShellArg stateDirectory}
    database_path=${lib.escapeShellArg databasePath}

    ${pkgs.coreutils}/bin/mkdir -p "$state_directory"
    ${pkgs.coreutils}/bin/rm -f \
      ${lib.escapeShellArg socketPath} \
      ${lib.escapeShellArg metaSocketPath}
    ${combinedPackage}/bin/spirit-migrate-store \
      "($database_path)"
  '';

  initializeJudgeState = pkgs.writeShellScript "spirit-judge-startup-state" ''
    set -eu

    ${pkgs.coreutils}/bin/mkdir -p ${lib.escapeShellArg stateDirectory}
    ${pkgs.coreutils}/bin/rm -f ${lib.escapeShellArg judgeSocketPath}
  '';

  judgeServeRequest =
    "(Serve (${judgeSocketPath} ${judgeConfigPackage} OpenAiCodex ${judgeModel} "
    + "(Some ${judgeReasoningEffort}) ${toString judgeTimeoutMilliseconds} None None "
    + "(Some ${judgeSessionReference}) (Some ${pkgs.util-linux}/bin/setsid) "
    + "(Some ${judgeProviderPackage}/bin/codex) None))";

  daemonServiceWrapper = pkgs.writeShellScriptBin "spirit-daemon-service" ''
    set -eu

    exec ${combinedPackage}/bin/spirit-daemon \
      ${daemonConfiguration}/${configurationPath}
  '';

  judgeServiceWrapper = pkgs.writeShellScriptBin "spirit-judge-daemon-service" ''
    set -eu

    exec ${judgePackage}/bin/spirit-judge \
      ${lib.escapeShellArg judgeServeRequest}
  '';

  commandLineWrapper = pkgs.writeShellScriptBin "spirit" ''
    export SPIRIT_SOCKET=${lib.escapeShellArg socketPath}
    exec ${combinedPackage}/bin/spirit "$@"
  '';

  metaSpiritCommandLineWrapper = pkgs.writeShellScriptBin "meta-spirit" ''
    export SPIRIT_META_SOCKET=${lib.escapeShellArg metaSocketPath}
    exec ${combinedPackage}/bin/meta-spirit "$@"
  '';
in
assert lib.assertMsg stateDirectoryIsAbsolute
  "spirit mkUserServiceArtifacts: stateDirectory must be absolute";
assert lib.assertMsg stateDirectoryIsDotosAtom
  "spirit mkUserServiceArtifacts: stateDirectory must be one DOTOS atom";
{
  paths = {
    inherit
      stateDirectory
      socketPath
      metaSocketPath
      judgeSocketPath
      databasePath
      configurationPath
      ;
  };
  packages = {
    spirit = combinedPackage;
    judge = judgePackage;
    judgeConfig = judgeConfigPackage;
    judgeProvider = judgeProviderPackage;
  };
  inherit
    daemonConfiguration
    activateState
    initializeState
    initializeJudgeState
    daemonServiceWrapper
    judgeServiceWrapper
    commandLineWrapper
    metaSpiritCommandLineWrapper
    ;
}
