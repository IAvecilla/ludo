use std::time::SystemTime;

use macroquad::prelude::*;

use crate::game::{Game, Keys, Reload, Rgba, Shape};

/// The longest a step can be: after a pause, as a reload makes, the game
/// goes on from where it was instead of jumping ahead.
const MAX_DT: f64 = 1.0 / 30.0;

pub fn play(game: Game, path: String) {
    let config = game.config();
    let conf = Conf {
        window_title: config.title.clone(),
        window_width: config.width,
        window_height: config.height,
        window_resizable: false,
        ..Default::default()
    };
    macroquad::Window::from_config(conf, run(game, path));
}

async fn run(mut game: Game, path: String) {
    let mut shapes = Vec::new();
    let mut failure: Option<String> = None;
    let mut reload_errors: Option<String> = None;
    let mut modified = modified_at(&path);

    loop {
        let now = modified_at(&path);
        if now != modified {
            modified = now;
            let reloaded = std::fs::read_to_string(&path)
                .map_err(|err| vec![format!("could not read `{path}`: {err}")])
                .and_then(|source| game.reload(&source));
            match reloaded {
                Ok(reload) => {
                    eprintln!(
                        "reloaded `{path}`{}",
                        match reload {
                            Reload::Kept => "",
                            Reload::Restarted => ", and restarted: a struct changed",
                        }
                    );
                    reload_errors = None;
                    failure = None;
                }
                Err(errors) => {
                    let errors = errors.join("\n");
                    eprintln!("{errors}");
                    reload_errors = Some(errors);
                }
            }
        }

        if is_key_pressed(KeyCode::F5) {
            match game.restart() {
                Ok(()) => failure = None,
                Err(error) => failure = Some(error),
            }
        }

        // One step for each frame the screen shows, over the time the last
        // one took, at whatever rate the screen refreshes.
        if failure.is_none() {
            let keys = Keys {
                held: keys(is_key_down).collect(),
                pressed: keys(is_key_pressed).collect(),
            };
            let dt = f64::from(get_frame_time()).min(MAX_DT);
            if let Err(error) = game.step(&keys, dt) {
                eprintln!("{error}");
                failure = Some(error);
            }
        }

        if failure.is_none() {
            match game.draw() {
                Ok(drawn) => shapes = drawn,
                Err(error) => {
                    eprintln!("{error}");
                    failure = Some(error);
                }
            }
        }

        clear_background(BLACK);
        for shape in &shapes {
            draw(shape);
        }
        if let Some(message) = reload_errors.as_ref().or(failure.as_ref()) {
            let lines: Vec<&str> = message.lines().collect();
            draw_rectangle(
                0.0,
                0.0,
                screen_width(),
                16.0 + 24.0 * lines.len() as f32,
                Color::from_rgba(0, 0, 0, 200),
            );
            for (i, line) in lines.iter().enumerate() {
                draw_text(line, 10.0, 26.0 + 24.0 * i as f32, 20.0, RED);
            }
        }

        next_frame().await;
    }
}

const KEYS: [(KeyCode, &str); 37] = [
    (KeyCode::A, "a"),
    (KeyCode::B, "b"),
    (KeyCode::C, "c"),
    (KeyCode::D, "d"),
    (KeyCode::E, "e"),
    (KeyCode::F, "f"),
    (KeyCode::G, "g"),
    (KeyCode::H, "h"),
    (KeyCode::I, "i"),
    (KeyCode::J, "j"),
    (KeyCode::K, "k"),
    (KeyCode::L, "l"),
    (KeyCode::M, "m"),
    (KeyCode::N, "n"),
    (KeyCode::O, "o"),
    (KeyCode::P, "p"),
    (KeyCode::Q, "q"),
    (KeyCode::R, "r"),
    (KeyCode::S, "s"),
    (KeyCode::T, "t"),
    (KeyCode::U, "u"),
    (KeyCode::V, "v"),
    (KeyCode::W, "w"),
    (KeyCode::X, "x"),
    (KeyCode::Y, "y"),
    (KeyCode::Z, "z"),
    (KeyCode::Up, "up"),
    (KeyCode::Down, "down"),
    (KeyCode::Left, "left"),
    (KeyCode::Right, "right"),
    (KeyCode::Space, "space"),
    (KeyCode::Enter, "enter"),
    (KeyCode::Escape, "escape"),
    (KeyCode::Tab, "tab"),
    (KeyCode::Backspace, "backspace"),
    (KeyCode::LeftShift, "shift"),
    (KeyCode::LeftControl, "control"),
];

fn modified_at(path: &str) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The names of the keys that are down, or that went down in this frame.
fn keys(is: fn(KeyCode) -> bool) -> impl Iterator<Item = &'static str> {
    KEYS.iter()
        .filter(move |(code, _)| is(*code))
        .map(|(_, name)| *name)
}

fn draw(shape: &Shape) {
    match shape {
        Shape::Rect { x, y, w, h, color } => draw_rectangle(*x, *y, *w, *h, rgb(*color)),
        Shape::Circle { x, y, r, color } => draw_circle(*x, *y, *r, rgb(*color)),
        Shape::Line {
            from,
            to,
            thickness,
            color,
        } => draw_line(from.0, from.1, to.0, to.1, *thickness, rgb(*color)),
        Shape::Text {
            x,
            y,
            size,
            text,
            color,
        } => {
            draw_text(text, *x, *y, *size, rgb(*color));
        }
    }
}

fn rgb(Rgba(r, g, b, a): Rgba) -> Color {
    Color::from_rgba(r, g, b, a)
}
