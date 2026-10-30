{ pkgs, appVersion }:
let
  source = ../../browser/engine;
  platforms = builtins.fromJSON (builtins.readFile (source + /runtime-platforms.json));
  target = pkgs.stdenv.hostPlatform.rust.rustcTarget;
  runtime = platforms.targets.${target};
  archive = pkgs.fetchurl {
    url = "https://nodejs.org/dist/v${platforms.node_version}/${runtime.file}";
    sha256 = runtime.sha256;
  };
  npmDeps = pkgs.importNpmLock { npmRoot = source; };
in
pkgs.stdenv.mkDerivation {
  pname = "cww-browser-engine";
  version = appVersion;
  src = source;
  dontBuild = true;
  dontPatchShebangs = true;
  # No implicit strip/RPATH/shebang rewrite after the manifest is sealed.
  dontFixup = true;
  nativeBuildInputs = [ pkgs.python3 pkgs.nodejs ]
    ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.patchelf ]
    ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.rcodesign ];
  configurePhase = ''
    runHook preConfigure
    export HOME="$TMPDIR/npm-home"
    mkdir -p "$HOME"
    cp ${npmDeps}/package.json ${npmDeps}/package-lock.json .
    # No generic npm hooks: those rebuild packages and rewrite shebangs.
    npm ci --offline --ignore-scripts --omit=dev --bin-links=false --no-audit --no-fund
    runHook postConfigure
  '';
  installPhase = ''
    runHook preInstall
    mkdir -p "$out/notices"
    # importNpmLock rewrites build-time resolution. Ship the original locks
    # and unchanged registry module sources; no links to mutable npm state.
    cp ${source}/{entry.mjs,engine.mjs,package.json,package-lock.json} "$out/"
    cp -R node_modules "$out/node_modules"
    cp -R notices/. "$out/notices/"
    tar -xOf ${archive} ${runtime.directory}/${runtime.executable} > "$out/node"
    tar -xOf ${archive} ${runtime.directory}/LICENSE > "$out/notices/Node-LICENSE"
    chmod -R u+w "$out"
    chmod 755 "$out/node"
  '' + pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
    patchelf --set-interpreter "$(cat $NIX_CC/nix-support/dynamic-linker)" \
      --set-rpath ${pkgs.lib.makeLibraryPath [ pkgs.stdenv.cc.cc.lib pkgs.glibc ]} "$out/node"
  '' + pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isDarwin ''
    rcodesign sign --entitlements-xml-file ${../macos/browser-engine.entitlements.plist} "$out/node"
  '' + ''
    python ${source}/scripts/package.py manifest "$out" --target ${target} --app-version ${appVersion}
    python ${source}/scripts/package.py validate "$out"
    runtime=$(python ${source}/scripts/package.py path "$out")
    env -u NODE_OPTIONS -u NODE_PATH "$runtime/node" --no-addons --no-global-search-paths --version
    runHook postInstall
  '';
}
