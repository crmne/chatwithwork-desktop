#!/usr/bin/env bash
# Stage the tray's AppIndicator libraries for the Flatpak bundle.
#
#   appindicator.sh TARGET OUTPUT    # TARGET: x86_64- or aarch64-unknown-linux-gnu
#
# The desktop app loads libayatana-appindicator3 for its tray item and stops
# without it, and the GNOME runtime doesn't carry it. These are Ubuntu
# 24.04's builds (glibc 2.39, below the runtime's), pinned by the SHA-256
# that apt verified against Ubuntu's signed index. They're extracted into
# OUTPUT/lib and OUTPUT/doc (their copyright files), which
# native-packages.yaml installs into the Flatpak only.
set -euo pipefail
target="${1:?target}"
out="${2:?output directory}"
case "$target" in
  x86_64-unknown-linux-gnu)
    arch=amd64 base=http://archive.ubuntu.com/ubuntu
    debs=(
      "pool/main/liba/libayatana-appindicator/libayatana-appindicator3-1_0.5.93-1build3_amd64.deb 1d34a37a9782176d0845540a03da328301cfbffbd465b5a07f9fe025625a4075"
      "pool/main/liba/libayatana-indicator/libayatana-indicator3-7_0.9.4-1build1_amd64.deb b3d7ac6dc1808086709960891f8070f072a97e8de0dba973edbf168ba31b8820"
      "pool/main/a/ayatana-ido/libayatana-ido3-0.4-0_0.10.1-1build2_amd64.deb 2b2b7a68a1d1f2668bae0dc56b0fcc2e3b220f762d9aec3c2828f15779105c36"
      "pool/main/libd/libdbusmenu/libdbusmenu-glib4_18.10.20180917~bzr492+repack1-3.1ubuntu5_amd64.deb 50697e24febeeafc896ffecbb9dc811b0d576f0e14fa2c5ded37f25e007b0dfc"
      "pool/main/libd/libdbusmenu/libdbusmenu-gtk3-4_18.10.20180917~bzr492+repack1-3.1ubuntu5_amd64.deb be9211290e51d056a1ff2234f311efdb21cc3adf85cd28f0ca080aaad1de3fad"
    ) ;;
  aarch64-unknown-linux-gnu)
    arch=arm64 base=http://ports.ubuntu.com/ubuntu-ports
    debs=(
      "pool/main/liba/libayatana-appindicator/libayatana-appindicator3-1_0.5.93-1build3_arm64.deb a46d9c097be81bd7aa428babd94f20c300800ff87f9d469495bde30f3027eb95"
      "pool/main/liba/libayatana-indicator/libayatana-indicator3-7_0.9.4-1build1_arm64.deb 051df6f95ccda3c5af0ef109e6f065440017295e32110c4e128d895ad094f2b3"
      "pool/main/a/ayatana-ido/libayatana-ido3-0.4-0_0.10.1-1build2_arm64.deb 352e6dd9fb99e3094628a0ddd5d4d3f5f5351c4d0176380095e879c621cf5527"
      "pool/main/libd/libdbusmenu/libdbusmenu-glib4_18.10.20180917~bzr492+repack1-3.1ubuntu5_arm64.deb 2e6a87d0ea8c85274aaeab4f56ec22a692859619cb9f32a3052950ea39664916"
      "pool/main/libd/libdbusmenu/libdbusmenu-gtk3-4_18.10.20180917~bzr492+repack1-3.1ubuntu5_arm64.deb f8c0bf1d8ebf4ab7e7c2440a76cb49baaad9689de5e6c1cdcd9b78698fd45a6a"
    ) ;;
  *) echo "unsupported target: $target" >&2; exit 2 ;;
esac
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
rm -rf "$out" && mkdir -p "$out/lib" "$out/doc"
for entry in "${debs[@]}"; do
  read -r path sha <<<"$entry"
  deb="$work/$(basename "$path")"
  curl -fsSL --retry 3 -o "$deb" "$base/$path"
  echo "$sha  $deb" | sha256sum -c --quiet -
  name="$(basename "$path" | cut -d_ -f1)"
  mkdir -p "$work/x/$name"
  (cd "$work/x/$name" && ar p "$deb" "$(ar t "$deb" | grep '^data\.tar')" | tar -x --zstd -f - 2>/dev/null ||
    ar p "$deb" "$(ar t "$deb" | grep '^data\.tar')" | tar -x -f -)
  cp -P "$work/x/$name"/usr/lib/*-linux-gnu/*.so.* "$out/lib/"
  install -Dm644 "$work/x/$name/usr/share/doc/$name/copyright" "$out/doc/$name/copyright"
done
ls "$out/lib"
