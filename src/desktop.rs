use eyre::{Context, Result};
use freedesktop_file_parser::{DesktopFile, EntryType};
use palette::{IntoColor, Oklab, Oklaba, color_difference::EuclideanDistance};
use std::{collections::HashMap, ffi::OsStr, fs::DirEntry, path::Path};

pub struct DesktopEntries {
    entries: Vec<DesktopEntry>,
}

pub struct DesktopEntry {
    pub _id: String,
    pub file: DesktopFile,
    pub avg_icon_color: Oklab,
}

impl DesktopEntries {
    pub fn count(&self) -> usize {
        self.entries.len()
    }
    pub fn colors(&self) -> impl Iterator<Item = Oklab> + ExactSizeIterator {
        self.entries.iter().map(|entry| entry.avg_icon_color)
    }
    pub fn find_entry(&self, color: Oklab) -> Option<&DesktopEntry> {
        self.entries
            .iter()
            .min_by_key(|x| OrdFloat(diff_color(x.avg_icon_color, color)))
    }
}

// keep it in sync with the gpu implementation
fn diff_color(icon: Oklab, color: Oklab) -> f32 {
    icon.distance_squared(color)
}

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

pub(crate) fn find_desktop_files() -> Result<DesktopEntries> {
    // https://specifications.freedesktop.org/desktop-entry/latest/file-naming.html
    let paths = std::env::var("XDG_DATA_DIRS").unwrap_or("/usr/local/share/:/usr/share/".into());
    let paths = std::env::split_paths(&paths);
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

            if !results.contains_key(&id)
                && file.entry.no_display != Some(true)
                && file.entry.hidden != Some(true)
                && let EntryType::Application(_) = file.entry.entry_type
                && let Some(icon) = &file.entry.icon
                && let Some(icon) = icon.get_icon_path()
                && icon.extension() != Some(OsStr::new("svg"))
            {
                let icon: image::DynamicImage = image::ImageReader::open(&icon)
                    .wrap_err_with(|| format!("{}", icon.display()))?
                    .decode()
                    .wrap_err_with(|| format!("decoding {}", icon.display()))?;
                let color = average_color(&icon);
                results.insert(
                    id.clone(),
                    DesktopEntry {
                        _id: id,
                        file,
                        avg_icon_color: color,
                    },
                );
            }

            Ok(())
        })
        .wrap_err_with(|| format!("{}", base.display()))?;
    }

    let mut entries = results.into_values().collect::<Vec<_>>();

    entries.sort_by_key(|entry| {
        let (l, a, b) = entry.avg_icon_color.into_components();
        (OrdFloat(a), OrdFloat(b), OrdFloat(l))
    });

    Ok(DesktopEntries { entries })
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

struct OrdFloat(f32);

impl PartialEq for OrdFloat {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for OrdFloat {}
impl PartialOrd for OrdFloat {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(&other))
    }
}
impl Ord for OrdFloat {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}
