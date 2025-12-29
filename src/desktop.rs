use eyre::{Context, Result};
use freedesktop_file_parser::{DesktopFile, EntryType};
use palette::{IntoColor, Oklab, Oklaba};
use std::{
    collections::HashMap,
    ffi::OsStr,
    fs::DirEntry,
    path::{Path, PathBuf},
};

fn walkdir(path: &Path, f: &mut impl FnMut(&DirEntry) -> Result<()>) -> Result<()> {
    for entry in path.read_dir()? {
        let entry = entry?;
        f(&entry).wrap_err_with(|| format!("{}", entry.path().display()))?;
        if entry.file_type()?.is_dir() {
            walkdir(&entry.path(), f).wrap_err_with(|| format!("{}", path.display()))?;
        }
    }
    Ok(())
}

pub(crate) fn find_desktop_files() -> Result<Vec<(DesktopFile, Oklab)>> {
    // https://specifications.freedesktop.org/desktop-entry/latest/file-naming.html
    let paths = std::env::var("XDG_DATA_DIRS").unwrap_or("/usr/local/share/:/usr/share/".into());
    let paths = std::env::split_paths(&paths).map(PathBuf::from);
    let mut results = HashMap::new();

    for data_dir in paths {
        let base = data_dir.join("applications");
        if !base.try_exists()? {
            continue;
        }
        walkdir(&base, &mut |file| {
            if file.path().extension() != Some(OsStr::new("desktop")) {
                return Ok(());
            }
            let path = file.path();
            let id = path
                .strip_prefix(&base)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('/', "-");

            let contents = std::fs::read_to_string(&path)?;

            let file =
                freedesktop_file_parser::parse(&contents).wrap_err("parsing .desktop file")?;

            if !results.contains_key(&id) {
                if file.entry.no_display != Some(true)
                    && file.entry.hidden != Some(true)
                    && let EntryType::Application(_) = file.entry.entry_type
                    && let Some(icon) = &file.entry.icon
                    && let Some(icon) = icon.get_icon_path()
                    && icon.extension() != Some(OsStr::new("svg"))
                { dbg!(path);
                    let icon: image::DynamicImage = image::ImageReader::open(&icon)
                        .wrap_err_with(|| format!("{}", icon.display()))?
                        .decode()
                        .wrap_err_with(|| format!("decoding {}", icon.display()))?;
                    let color = average_color(&icon);
                    results.insert(id, (file, color));
                }
            }

            Ok(())
        })
        .wrap_err_with(|| format!("{}", base.display()))?;
    }

    Ok(results.into_values().collect())
}

fn average_color(image: &image::DynamicImage) -> palette::Oklab {
    use palette::cast::FromComponents;

    let mut total_l = 0.0;
    let mut total_a = 0.0;
    let mut total_b = 0.0;

    let image = image.to_rgba8();
    let pixels = <&[palette::Srgba<u8>]>::from_components(&*image);

    let mut count = 0.0;
    for pixel in pixels {
        let color: Oklaba = pixel.into_linear().into_color();

        let weight = color.alpha;
        total_l += color.l * weight;
        total_a += color.a * weight;
        total_b += color.b * weight;

        count += weight;
    }

    Oklab {
        l: total_l / count,
        a: total_a / count,
        b: total_b / count,
    }
}
