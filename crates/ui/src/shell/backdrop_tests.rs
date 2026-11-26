//! LLM-566: background image config and layout.

use std::path::Path;

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
    let (b, problems) = Backdrops::resolve(&config, Some(Path::new("/cfg")));
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
    assert_eq!(Backdrops::resolve(&config, None).0, Backdrops::default());
}

#[test]
fn opacity_is_clamped() {
    let config: BackgroundsConfig = toml::from_str("[list]\nimage = \"a\"\nopacity = 3").unwrap();
    assert_eq!(
        Backdrops::resolve(&config, None).0.list.unwrap().opacity,
        1.
    );
}
