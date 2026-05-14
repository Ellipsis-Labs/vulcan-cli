{
  description = "A dev shell Nix flake for vulcan";
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs.url = "github:nixos/nixpkgs/nixos-25.11";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    rust-overlay,
    flake-utils,
    nixpkgs,
    ...
  }: let
  in
    flake-utils.lib.eachDefaultSystem
    (
      system: let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [rust-overlay.overlays.default];
        };
      in
        with pkgs; rec
        {
          # Function to create a cross-build shell for a specific target
          mkCrossBuildShell = targetSystem: let
            # Define rust targets for each platform
            rustTarget =
              if targetSystem == "x86_64-linux"
              then "x86_64-unknown-linux-musl"
              else if targetSystem == "aarch64-linux"
              then "aarch64-unknown-linux-musl"
              else if targetSystem == "x86_64-darwin"
              then "x86_64-apple-darwin"
              else if targetSystem == "aarch64-darwin"
              then "aarch64-apple-darwin"
              else throw "Unsupported target system: ${targetSystem}";

            isLinux = lib.hasSuffix "linux" targetSystem;
            isDarwin = lib.hasSuffix "darwin" targetSystem;

            # Import cross packages
            # If target equals build system, don't set up cross-compilation
            pkgsCross =
              if targetSystem == system
              then pkgs
              else
                import nixpkgs {
                  inherit system;
                  crossSystem = {
                    config =
                      if targetSystem == "x86_64-linux"
                      then "x86_64-unknown-linux-musl"
                      else if targetSystem == "aarch64-linux"
                      then "aarch64-unknown-linux-musl"
                      else if targetSystem == "x86_64-darwin"
                      then "x86_64-apple-darwin"
                      else if targetSystem == "aarch64-darwin"
                      then "aarch64-apple-darwin"
                      else throw "Unsupported target system: ${targetSystem}";
                  };
                };

            inherit (pkgs) lib;
            # Use pkgsStatic for static libraries on all platforms
            pkgsStatic = pkgsCross.pkgsStatic;
            stdenv = pkgsStatic.libcxxStdenv;

            # Rust toolchain
            toolchain = pkgs.rust-bin.stable.latest.default.override {
              targets = [rustTarget];
            };

            # Convert rust target to env var name (e.g., x86_64-unknown-linux-musl -> x86_64_unknown_linux_musl)
            envVarName = builtins.replaceStrings ["-"] ["_"] rustTarget;
          in
            mkShell {
              buildInputs =
                [
                ]
                ++ lib.optionals stdenv.isDarwin [
                  pkgsStatic.libiconv
                ];

              nativeBuildInputs = [
                toolchain
              ];

              CARGO_INCREMENTAL = 0; # disable incremental compilation
              RUSTFLAGS =
                # https://github.com/rust-lang/cargo/issues/4133
                "-C linker=${stdenv.cc}/bin/${stdenv.cc.targetPrefix}ld"
                + (
                  if stdenv.isDarwin
                  then " -L ${pkgsStatic.libiconv.dev}/lib"
                  else " -C link-arg=-static -C target-feature=+crt-static"
                );

              # Static library configuration
              LIBICONV_STATIC = lib.optionalString stdenv.isDarwin "1";
              OPENSSL_STATIC = "1";
              OPENSSL_LIB_DIR = "${pkgsStatic.openssl.out}/lib";
              OPENSSL_INCLUDE_DIR = "${pkgsStatic.openssl.dev}/include";

              shellHook = ''
                echo "Cross-compilation shell for ${targetSystem}"
                echo "Target: ${rustTarget}"
                echo ""
                ${
                  if isLinux
                  then ''
                    echo "Building statically linked binaries with musl"
                  ''
                  else if isDarwin
                  then ''
                    echo "Building for macOS (system libraries dynamic linking, everything else static)"
                  ''
                  else ""
                }
                echo ""
                echo "Key environment variables:"
                echo "  CARGO_BUILD_TARGET: $CARGO_BUILD_TARGET"
                echo "  RUSTFLAGS: $RUSTFLAGS"
                echo "  CARGO_INCREMENTAL: $CARGO_INCREMENTAL"
                echo "  RUSTC_WRAPPER: $RUSTC_WRAPPER"
                echo "  CC: $CC"
                echo "  CXX: $CXX"
                echo "  CC_${envVarName}: ''${CC_${envVarName}}"
                echo "  CXX_${envVarName}: ''${CXX_${envVarName}}"
                echo ""
                echo "To build binaries:"
                echo "  cargo build --release"
                echo ""
                echo "The resulting binaries will be in:"
                echo "  target/${rustTarget}/release/"
              '';
              # Cross-compilation environment variables
              CARGO_BUILD_TARGET = rustTarget;
              "CC_${envVarName}" = "${stdenv.cc}/bin/${stdenv.cc.targetPrefix}cc";
              "CXX_${envVarName}" = "${stdenv.cc}/bin/${stdenv.cc.targetPrefix}c++";
              "AR_${envVarName}" = "${stdenv.cc.bintools.bintools}/bin/${stdenv.cc.targetPrefix}ar";
            };

          # Create dev shells for each target
          devShells = {
            "crossBuildShell-x86_64-linux" = mkCrossBuildShell "x86_64-linux";
            "crossBuildShell-aarch64-linux" = mkCrossBuildShell "aarch64-linux";
            "crossBuildShell-x86_64-darwin" = mkCrossBuildShell "x86_64-darwin";
            "crossBuildShell-aarch64-darwin" = mkCrossBuildShell "aarch64-darwin";
          };
        }
    );
}
