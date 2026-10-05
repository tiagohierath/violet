{
  description = "Lilyfield: a drawing app";

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
        pname = "lilyfield";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = [ pkgs.pkg-config ];
        buildInputs = runtimeLibs;
        doCheck = false;
        postInstall = ''
          mkdir -p $out/share/applications
          cat > $out/share/applications/lilyfield.desktop <<EOF
          [Desktop Entry]
          Type=Application
          Name=Lilyfield
          Exec=lilyfield %F
          Terminal=false
          Categories=Graphics;2DGraphics;RasterGraphics;
          EOF
        '';
        # winit and wgpu dlopen these libs; fixup's shrink-rpath drops them, so add them back.
        postFixup = ''
          patchelf --add-rpath ${pkgs.lib.makeLibraryPath runtimeLibs} $out/bin/*
        '';
      };
    };
}
