//! LLM-566: background image config and layout.

use std::path::Path;

use super::backdrop::load;
use super::backdrop::*;

fn area(w: f32, h: f32) -> Rect {
    Rect { x: 0., y: 0., w, h }
}

fn layout(size: BgSize, pos: &str, repeat: &str) -> Layout {
    Layout {
        size,
        position: parse_position(pos).unwrap(),
        repeat: parse_repeat(repeat).unwrap(),
    }
}

#[test]
fn cover_fills_the_box_and_centers() {
    let t = tiles(area(200., 100.), (100., 100.), &Layout::default());
    assert_eq!(
        t,
        vec![Rect {
            x: 0.,
            y: -50.,
            w: 200.,
            h: 200.
        }]
    );
}

#[test]
fn contain_fits_inside() {
    let t = tiles(
        area(200., 100.),
        (100., 100.),
        &layout(BgSize::Contain, "center", "no-repeat"),
    );
    assert_eq!(
        t,
        vec![Rect {
            x: 50.,
            y: 0.,
            w: 100.,
            h: 100.
        }]
    );
}

#[test]
fn fill_stretches() {
    let t = tiles(
        area(200., 100.),
        (10., 30.),
        &layout(BgSize::Fill, "center", "no-repeat"),
    );
    assert_eq!(
        t,
        vec![Rect {
            x: 0.,
            y: 0.,
            w: 200.,
            h: 100.
        }]
    );
}

#[test]
fn position_keywords_and_percents() {
    let l = layout(BgSize::Scale(1.), "bottom right", "no-repeat");
    assert_eq!(
        tiles(area(100., 100.), (20., 10.), &l),
        vec![Rect {
            x: 80.,
            y: 90.,
            w: 20.,
            h: 10.
        }]
    );
    let l = layout(BgSize::Scale(1.), "25% 10px", "no-repeat");
    assert_eq!(
        tiles(area(100., 100.), (20., 10.), &l),
        vec![Rect {
            x: 20.,
            y: 10.,
            w: 20.,
            h: 10.
        }]
    );
}

#[test]
fn one_position_value_centers_the_other_axis() {
    assert_eq!(
        parse_position("top"),
        Some((Offset::Share(0.5), Offset::Share(0.)))
    );
    assert_eq!(
        parse_position("left"),
        Some((Offset::Share(0.), Offset::Share(0.5)))
    );
    assert_eq!(
        parse_position("30%"),
        Some((Offset::Share(0.3), Offset::Share(0.5)))
    );
    assert_eq!(parse_position("left left"), None);
    assert_eq!(parse_position("middle"), None);
}

#[test]
fn repeat_covers_the_box_from_the_position() {
    // A 30 px tile centred in 100 px starts at 35, so tiles start at -25.
    let l = layout(BgSize::Scale(1.), "center", "repeat-x");
    let xs: Vec<f32> = tiles(area(100., 30.), (30., 30.), &l)
        .iter()
        .map(|r| r.x)
        .collect();
    assert_eq!(xs, vec![-25., 5., 35., 65., 95.]);
    let l = layout(BgSize::Scale(1.), "top left", "repeat");
    assert_eq!(tiles(area(100., 100.), (50., 50.), &l).len(), 4);
}

#[test]
fn tiny_tiles_stop_at_the_cap() {
    let l = layout(BgSize::Scale(1.), "top left", "repeat");
    assert_eq!(tiles(area(4000., 4000.), (1., 1.), &l).len(), MAX_TILES);
}

#[test]
fn sizes_parse_like_css() {
    let s = |t: &str| parse_size(&SizeValue::Text(t.into()));
    assert_eq!(s("auto"), Some(BgSize::Scale(1.)));
    assert_eq!(s("40%"), Some(BgSize::WidthShare(0.4)));
    assert_eq!(s("320px"), Some(BgSize::Width(320.)));
    assert_eq!(s("0.5"), Some(BgSize::Scale(0.5)));
    assert_eq!(parse_size(&SizeValue::Scale(2.)), Some(BgSize::Scale(2.)));
    assert_eq!(s("huge"), None);
    assert_eq!(s("-1"), None);
}

#[test]
fn width_sizes_keep_the_aspect() {
    let l = layout(BgSize::WidthShare(0.5), "top left", "no-repeat");
    assert_eq!(
        tiles(area(200., 200.), (100., 50.), &l),
        vec![Rect {
            x: 0.,
            y: 0.,
            w: 100.,
            h: 50.
        }]
    );
}

#[test]
fn the_config_reads_from_toml() {
    let text = r#"
        [window]
        image = "wall.jpg"
        opacity = 0.4
        size = 0.5

        [sidebar]
        image = "/abs/tile.png"
        size = "auto"
        repeat = "repeat"
        position = "top left"

        [task]
        image = "x.png"
        repeat = "sideways"
    "#;
    let config: BackgroundsConfig = toml::from_str(text).unwrap();
    let (b, problems) =
        Backdrops::resolve(&config, Some(Path::new("/cfg")), &Pick::default(), false);
    let window = b.window.unwrap();
    assert_eq!(window.image, Path::new("/cfg/wall.jpg"));
    assert_eq!(window.opacity, 0.4);
    assert_eq!(window.layout.size, BgSize::Scale(0.5));
    let sidebar = b.sidebar.unwrap();
    assert_eq!(sidebar.image, Path::new("/abs/tile.png"));
    assert_eq!(sidebar.opacity, DEFAULT_OPACITY);
    assert_eq!(sidebar.layout.repeat, Repeat { x: true, y: true });
    assert!(b.list.is_none());
    // A bad value falls back and says so.
    assert_eq!(b.task.unwrap().layout.repeat, Repeat { x: false, y: false });
    assert_eq!(
        problems,
        vec!["background.task: unknown repeat \"sideways\"".to_string()]
    );
}

#[test]
fn an_empty_image_turns_the_slot_off() {
    let config: BackgroundsConfig = toml::from_str("[list]\nimage = \"\"\nopacity = 1").unwrap();
    assert_eq!(
        Backdrops::resolve(&config, None, &Pick::default(), false).0,
        Backdrops::default()
    );
}

#[test]
fn opacity_is_clamped() {
    let config: BackgroundsConfig = toml::from_str("[list]\nimage = \"a\"\nopacity = 3").unwrap();
    assert_eq!(
        Backdrops::resolve(&config, None, &Pick::default(), false)
            .0
            .list
            .unwrap()
            .opacity,
        1.
    );
}

// LLM-573: images shrink to the size they show at.

fn fit(size: BgSize) -> load::Fit {
    load::Fit {
        size,
        screen: (1728., 1117.),
        scale: 2.,
    }
}

fn key(path: &str, size: BgSize) -> load::ImageKey {
    load::ImageKey {
        path: path.into(),
        stamp: None,
        fit: fit(size),
        effects: load::Effects::default(),
    }
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 100, 50, 255]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

#[test]
fn a_large_photo_shrinks_to_the_screen() {
    // Cover on a 1728×1117 pt screen at 2x needs 3456 px across.
    let f = load::shrink_factor((8000., 6000.), &fit(BgSize::Cover));
    assert!((f - 0.432).abs() < 0.001, "{f}");
    let loaded = load::load_bytes(&png(8000, 6000), &key("a.png", BgSize::Cover)).unwrap();
    let s = loaded.image.size(0);
    assert_eq!((u32::from(s.width), u32::from(s.height)), (3456, 2592));
    // Layout still sees the file's own size.
    assert_eq!(loaded.natural, (8000., 6000.));
}

#[test]
fn a_small_image_is_never_enlarged() {
    assert_eq!(load::shrink_factor((400., 300.), &fit(BgSize::Cover)), 1.);
    let loaded = load::load_bytes(&png(40, 30), &key("a.png", BgSize::Cover)).unwrap();
    assert_eq!(u32::from(loaded.image.size(0).width), 40);
}

#[test]
fn a_tile_shrinks_to_its_own_size() {
    // A 1000 px tile at a quarter of its size shows at 250 pt, 500 px.
    let f = load::shrink_factor((1000., 1000.), &fit(BgSize::Scale(0.25)));
    assert_eq!(f, 0.5);
    let f = load::shrink_factor((1000., 1000.), &fit(BgSize::Width(100.)));
    assert_eq!(f, 0.2);
}

#[test]
fn an_image_past_the_gpu_limit_still_fits() {
    let f = load::shrink_factor((40000., 100.), &fit(BgSize::Scale(1.)));
    assert!(40000. * f <= load::MAX_TEXTURE as f32);
}

#[test]
fn a_file_that_is_not_an_image_fails_to_decode() {
    assert!(matches!(
        load::load_bytes(b"hello", &key("a.txt", BgSize::Cover)),
        Err(load::DecodeError::NotAnImage)
    ));
}

// LLM-572: blur, grayscale, brightness and tint.

#[test]
fn filters_read_from_toml() {
    let text = r##"
        [list]
        image = "a.png"
        blur = 12
        grayscale = true
        brightness = 0.6
        tint = "#1e1e2e"
        tint_opacity = 0.5

        [task]
        image = "b.png"
        tint = "purple-ish"
        blur = -3
    "##;
    let config: BackgroundsConfig = toml::from_str(text).unwrap();
    let (b, problems) = Backdrops::resolve(&config, None, &Pick::default(), false);
    let list = b.list.unwrap();
    assert_eq!(list.effects.blur, 12.);
    assert_eq!(list.effects.brightness, 0.6);
    assert!(list.grayscale);
    let tint = list.tint.unwrap();
    assert!((tint.a - 0.5).abs() < 0.001);
    let task = b.task.unwrap();
    assert_eq!(task.tint, None);
    assert_eq!(task.effects.blur, 0.);
    assert_eq!(
        problems,
        vec![
            "background.task: unknown blur \"-3\"".to_string(),
            "background.task: unknown tint \"purple-ish\"".to_string(),
        ]
    );
}

#[test]
fn brightness_scales_each_channel() {
    let image = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 100, 50, 255]));
    let out = load::apply_effects(image, 0., 0.5);
    assert_eq!(out.get_pixel(1, 1).0, [100, 50, 25, 255]);
}

#[test]
fn blur_softens_an_edge() {
    let mut image = image::RgbaImage::from_pixel(20, 1, image::Rgba([0, 0, 0, 255]));
    for x in 10..20 {
        image.put_pixel(x, 0, image::Rgba([255, 255, 255, 255]));
    }
    let out = load::apply_effects(image, 3., 1.);
    let edge = out.get_pixel(10, 0).0[0];
    assert!(edge > 0 && edge < 255, "{edge}");
}

#[test]
fn a_blurred_image_loads_with_its_own_key() {
    let mut blurred = key("a.png", BgSize::Cover);
    blurred.effects.blur = 8.;
    assert_ne!(blurred, key("a.png", BgSize::Cover));
    let loaded = load::load_bytes(&png(64, 64), &blurred).unwrap();
    // A flat image stays flat, and in BGRA.
    let bytes = loaded.image.as_bytes(0).unwrap();
    assert_eq!(&bytes[..4], &[50, 100, 200, 255]);
}

// LLM-568: a changed or missing file.

#[test]
fn a_missing_file_is_named_and_a_changed_one_gets_a_new_stamp() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.png"), png(4, 4)).unwrap();
    let config: BackgroundsConfig =
        toml::from_str("[window]\nimage = \"a.png\"\n[list]\nimage = \"gone.png\"").unwrap();
    let (b, _) = Backdrops::resolve(&config, Some(dir.path()), &Pick::default(), false);
    let first = b.window.clone().unwrap().stamp;
    assert!(first.is_some());
    assert_eq!(b.missing(), vec![(Slot::List, dir.path().join("gone.png"))]);
    // A later write moves the stamp on.
    let file = std::fs::File::options()
        .write(true)
        .open(dir.path().join("a.png"))
        .unwrap();
    file.set_modified(first.unwrap() + std::time::Duration::from_secs(5))
        .unwrap();
    assert_ne!(stamp_of(&dir.path().join("a.png")), first);
}

// LLM-571: an image per mode.

#[test]
fn each_mode_takes_its_own_image_and_opacity() {
    let text = r#"
        [window]
        image = "both.png"
        image_dark = "night.png"
        opacity = 0.2
        opacity_light = 0.5
    "#;
    let config: BackgroundsConfig = toml::from_str(text).unwrap();
    let (shown, _) = Shown::resolve(&config, Some(Path::new("/c")), &Pick::default());
    let dark = shown.dark.window.unwrap();
    assert_eq!(dark.image, Path::new("/c/night.png"));
    assert_eq!(dark.opacity, 0.2);
    let light = shown.light.window.unwrap();
    assert_eq!(light.image, Path::new("/c/both.png"));
    assert_eq!(light.opacity, 0.5);
}

#[test]
fn a_mode_image_alone_shows_only_in_that_mode() {
    let config: BackgroundsConfig = toml::from_str("[list]\nimage_dark = \"n.png\"").unwrap();
    let (shown, problems) = Shown::resolve(&config, None, &Pick::default());
    assert!(shown.dark.list.is_some());
    assert!(shown.light.list.is_none());
    assert!(problems.is_empty());
}

// LLM-574: per project and team.

#[test]
fn the_most_specific_table_wins_per_slot() {
    let text = r#"
        [window]
        image = "global.png"
        [list]
        image = "global-list.png"

        [team.WORK.window]
        image = "work.png"

        [project.site.list]
        image = "site-list.png"
    "#;
    let config: BackgroundsConfig = toml::from_str(text).unwrap();
    let get = |pick: Pick| Backdrops::resolve(&config, None, &pick, false).0;
    let name = |b: Option<Backdrop>| b.unwrap().image.to_string_lossy().into_owned();

    let all = get(Pick::default());
    assert_eq!(name(all.window), "global.png");

    let team = get(Pick {
        project: None,
        teams: vec!["work".into()],
    });
    assert_eq!(name(team.window), "work.png");
    assert_eq!(name(team.list), "global-list.png");

    // The project sets its list; its team still sets the window.
    let project = get(Pick {
        project: Some("site".into()),
        teams: vec!["WORK".into()],
    });
    assert_eq!(name(project.list), "site-list.png");
    assert_eq!(name(project.window), "work.png");
}
