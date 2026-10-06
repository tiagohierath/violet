{
  description = "Violet: a drawing app (formerly Lilyfield)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};

      runtimeLibs = with pkgs; [
        libGL
        libxkbcommon
        vulkan-loader
        wayland
      ];
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        nativeBuildInputs = with pkgs; [ pkg-config rustc cargo rustfmt clippy ];
        buildInputs = runtimeLibs;
        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;
      };

      packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
        pname = "violet";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        # Use the quick-to-rebuild release profile for this local install.
        buildType = "fast";
        nativeBuildInputs = [ pkgs.pkg-config ];
        buildInputs = runtimeLibs;
        doCheck = true;
        postInstall = ''
          mkdir -p $out/share/applications
          cat > $out/share/applications/violet.desktop <<EOF
          [Desktop Entry]
          Type=Application
          Name=Violet
          Exec=$out/bin/violet %f
          StartupWMClass=violet
          Terminal=false
          Categories=Graphics;2DGraphics;RasterGraphics;
          EOF
        '';
        # winit and wgpu dlopen these libs; fixup's shrink-rpath drops them, so add them back.
        postFixup = ''
          patchelf --add-rpath ${pkgs.lib.makeLibraryPath runtimeLibs} $out/bin/violet
        '';
      };
    };
}
