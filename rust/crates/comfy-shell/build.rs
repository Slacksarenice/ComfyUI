use std::env;
use std::path::{Path, PathBuf};

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let winmd_dir = crate_dir.join(".winappsdk").join("winmd");
    println!("cargo:rerun-if-changed={}", winmd_dir.display());
    println!("cargo:rerun-if-changed=build.rs");

    if !winmd_dir.exists() {
        panic!(
            "Windows App SDK metadata not found at {}.\n\
             Run rust/tools/fetch-winappsdk.ps1 first.",
            winmd_dir.display()
        );
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    // Generated into src/ (gitignored) because the file carries inner
    // attributes, which include! cannot splice into a module.
    let bindings = crate_dir.join("src").join("bindings.rs");

    let _ = windows_bindgen::bindgen([
        "--in",
        winmd_dir.to_str().unwrap(),
        "--in",
        "default",
        "--out",
        bindings.to_str().unwrap(),
        // The generator's built-in reference defaults map more types into the
        // split windows-collections/numerics crates than their released
        // versions export, so disable them and declare references explicitly.
        // First matching reference wins: specific entries come first.
        "--no-deps",
        "--reference",
        "windows_collections,flat,Windows.Foundation.Collections.IIterable",
        "--reference",
        "windows_collections,flat,Windows.Foundation.Collections.IIterator",
        "--reference",
        "windows_collections,flat,Windows.Foundation.Collections.IKeyValuePair",
        "--reference",
        "windows_collections,flat,Windows.Foundation.Collections.IMap",
        "--reference",
        "windows_collections,flat,Windows.Foundation.Collections.IMapView",
        "--reference",
        "windows_collections,flat,Windows.Foundation.Collections.IVector",
        "--reference",
        "windows_collections,flat,Windows.Foundation.Collections.IVectorView",
        "--reference",
        "windows_numerics,flat,Windows.Foundation.Numerics.Matrix3x2",
        "--reference",
        "windows_numerics,flat,Windows.Foundation.Numerics.Matrix4x4",
        "--reference",
        "windows_numerics,flat,Windows.Foundation.Numerics.Vector2",
        "--reference",
        "windows_numerics,flat,Windows.Foundation.Numerics.Vector3",
        "--reference",
        "windows_numerics,flat,Windows.Foundation.Numerics.Vector4",
        "--reference",
        "windows_future,flat,Windows.Foundation.Async*",
        "--reference",
        "windows_future,flat,Windows.Foundation.IAsync*",
        // Map remaining Windows.* onto the windows crate, except
        // Windows.UI.Xaml.* (removed from the windows crate), which is
        // generated locally. References are prefix filters, so Windows.UI
        // is listed per child namespace.
        "--reference",
        "windows,skip-root,Windows.ApplicationModel",
        "--reference",
        "windows,skip-root,Windows.Devices",
        "--reference",
        "windows,skip-root,Windows.Foundation",
        "--reference",
        "windows,skip-root,Windows.Globalization",
        "--reference",
        "windows,skip-root,Windows.Graphics",
        "--reference",
        "windows,skip-root,Windows.Media",
        "--reference",
        "windows,skip-root,Windows.Security",
        "--reference",
        "windows,skip-root,Windows.Storage",
        "--reference",
        "windows,skip-root,Windows.System",
        "--reference",
        "windows,skip-root,Windows.UI.Composition",
        "--reference",
        "windows,skip-root,Windows.UI.Core",
        "--reference",
        "windows,skip-root,Windows.UI.Text",
        "--reference",
        "windows,skip-root,Windows.UI.Color",
        "--reference",
        "windows,skip-root,Windows.UI.Colors",
        "--filter",
        "Microsoft.UI",
        "Microsoft.Graphics",
        "Microsoft.Web.WebView2.Core",
    ]);

    // Stage the bootstrap DLL next to the produced executable so unpackaged
    // (non-MSIX) runs can load the Windows App Runtime.
    let bootstrap = crate_dir
        .join(".winappsdk")
        .join("microsoft.windowsappsdk.foundation.2.1.0")
        .join("runtimes")
        .join("win-x64")
        .join("native")
        .join("Microsoft.WindowsAppRuntime.Bootstrap.dll");
    if bootstrap.exists() {
        if let Some(profile_dir) = target_profile_dir(&out_dir) {
            let dest = profile_dir.join("Microsoft.WindowsAppRuntime.Bootstrap.dll");
            let _ = std::fs::copy(&bootstrap, &dest);
        }
    }
}

// OUT_DIR is <target>/<profile>/build/<pkg>-<hash>/out; walk up to <profile>.
fn target_profile_dir(out_dir: &Path) -> Option<PathBuf> {
    out_dir.ancestors().nth(3).map(|p| p.to_path_buf())
}
