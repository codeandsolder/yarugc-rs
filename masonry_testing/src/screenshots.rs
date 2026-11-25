// Copyright 2022 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Helper functions for writing snapshot tests and comparing images.

use std::fs::{self, File};
use std::io::{BufReader, Cursor, ErrorKind};
use std::path::Path;

use crate::HarnessError;
use image::{DynamicImage, ImageFormat, ImageReader, RgbImage, RgbaImage};
use oxipng::{Options, optimize_from_memory};

fn get_image_diff(ref_image: &RgbImage, new_image: &RgbImage) -> Option<RgbImage> {
    if ref_image == new_image {
        return None;
    }

    let width = std::cmp::max(ref_image.width(), new_image.width());
    let height = std::cmp::max(ref_image.height(), new_image.height());

    let diff_image = RgbImage::from_fn(width, height, |x, y| {
        match (
            ref_image.get_pixel_checked(x, y),
            new_image.get_pixel_checked(x, y),
        ) {
            (Some(a), Some(b)) if a == b => [0, 0, 0].into(),
            (_, Some(pixel)) => *pixel,
            (Some(_), None) => [255, 255, 255].into(),
            (None, None) => [0, 0, 0].into(),
        }
    });

    Some(diff_image)
}

fn enforce_size(path: &Path, size: u64, limit: u32) -> Result<(), HarnessError> {
    if size > u64::from(limit) {
        Err(HarnessError::SnapshotTooLarge {
            path: path.to_owned(),
            size,
            limit,
        })
    } else {
        Ok(())
    }
}

fn save_image(image: &DynamicImage, path: &Path, limit: Option<u32>) -> Result<(), HarnessError> {
    let mut buffer = Cursor::new(Vec::new());
    image.write_to(&mut buffer, ImageFormat::Png)?;
    let data = optimize_from_memory(buffer.get_ref(), &Options::from_preset(5))?;
    let size = u64::try_from(data.len()).unwrap_or(u64::MAX);
    if let Some(limit) = limit {
        enforce_size(path, size, limit)?;
    }
    fs::write(path, data)?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), HarnessError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn compare_snapshot(
    image: RgbaImage,
    manifest_dir: &Path,
    test_name: &str,
    expect_failure: bool,
    limit: u32,
    bless: bool,
) -> Result<(), HarnessError> {
    let folder = manifest_dir.join("screenshots");
    let reference = folder.join(format!("{test_name}.png"));
    let new = folder.join(format!("{test_name}.new.png"));
    let diff = folder.join(format!("{test_name}.diff.png"));
    let reference_file = match File::open(&reference) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            if expect_failure {
                return Err(HarnessError::MissingReference(reference));
            }
            fs::create_dir_all(&folder)?;
            let image = DynamicImage::ImageRgba8(image);
            if bless {
                save_image(&image, &reference, Some(limit))?;
                remove_if_exists(&new)?;
                remove_if_exists(&diff)?;
                return Ok(());
            }
            save_image(&image, &new, Some(limit))?;
            return Err(HarnessError::MissingReference(reference));
        }
        Err(error) => return Err(error.into()),
    };

    let reference_size = reference_file.metadata()?.len();
    let reference_image =
        ImageReader::with_format(BufReader::new(reference_file), ImageFormat::Png)
            .decode()?
            .to_rgb8();
    let new_image = DynamicImage::ImageRgba8(image);
    let difference = get_image_diff(&reference_image, &new_image.to_rgb8());
    if expect_failure {
        return if difference.is_some() {
            Ok(())
        } else {
            Err(HarnessError::UnexpectedMatch(reference))
        };
    }
    if let Some(diff_image) = difference {
        if !bless {
            save_image(&new_image, &new, Some(limit))?;
            save_image(&diff_image.into(), &diff, None)?;
            return Err(HarnessError::SnapshotMismatch(reference));
        }
        save_image(&new_image, &reference, Some(limit))?;
    } else {
        enforce_size(&reference, reference_size, limit)?;
    }
    remove_if_exists(&new)?;
    remove_if_exists(&diff)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, Rgba};

    #[test]
    fn diff_detects_size_and_pixel_changes() {
        let black = RgbImage::from_pixel(2, 2, Rgb([0, 0, 0]));
        assert!(get_image_diff(&black, &black).is_none());
        let larger = RgbImage::from_pixel(3, 2, Rgb([0, 0, 0]));
        assert_eq!(
            get_image_diff(&black, &larger).unwrap().dimensions(),
            (3, 2)
        );
        let mut changed = black.clone();
        changed.put_pixel(1, 1, Rgb([255, 0, 0]));
        let diff = get_image_diff(&black, &changed).unwrap();
        assert_eq!(diff.get_pixel(1, 1), &Rgb([255, 0, 0]));
        assert_eq!(diff.get_pixel(0, 0), &Rgb([0, 0, 0]));
    }

    #[test]
    fn snapshot_expect_failure_is_read_only_and_missing_reference_is_distinct() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let original = RgbaImage::from_pixel(2, 2, Rgba([12, 34, 56, 255]));
        let changed = RgbaImage::from_pixel(2, 2, Rgba([56, 34, 12, 255]));
        let folder = root.join("screenshots");
        let path = folder.join("sample.png");

        assert!(matches!(
            compare_snapshot(original.clone(), root, "sample", true, 4096, true),
            Err(HarnessError::MissingReference(_))
        ));
        assert!(!folder.exists());
        compare_snapshot(original.clone(), root, "sample", false, 4096, true).unwrap();
        let saved = fs::read(&path).unwrap();

        assert!(matches!(
            compare_snapshot(original.clone(), root, "sample", true, 4096, true),
            Err(HarnessError::UnexpectedMatch(_))
        ));
        compare_snapshot(changed.clone(), root, "sample", true, 4096, true).unwrap();
        assert_eq!(fs::read(&path).unwrap(), saved);
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);

        assert!(matches!(
            compare_snapshot(changed.clone(), root, "sample", false, 1, true),
            Err(HarnessError::SnapshotTooLarge { .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), saved);

        assert!(matches!(
            compare_snapshot(changed, root, "sample", false, 4096, false),
            Err(HarnessError::SnapshotMismatch(_))
        ));
        assert!(folder.join("sample.new.png").exists());
        assert!(folder.join("sample.diff.png").exists());
        compare_snapshot(original, root, "sample", false, 4096, false).unwrap();
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
    }
}
