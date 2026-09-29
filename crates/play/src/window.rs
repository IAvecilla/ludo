use std::time::SystemTime;

use macroquad::prelude::*;

use crate::game::{Game, Keys, Reload, Rgba, Shape};

const DT: f64 = 1.0 / 60.0;
const MAX_STEPS_PER_FRAME: usize = 8;

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
    let mut elapsed = 0.0;
    let mut pending: Vec<&str> = Vec::new();
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
                    elapsed = 0.0;
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
                Ok(()) => {
                    failure = None;
                    elapsed = 0.0;
                }
                Err(error) => failure = Some(error),
            }
        }

        if failure.is_none() {
            elapsed += f64::from(get_frame_time());
            for key in just_pressed() {
                if !pending.contains(&key) {
                    pending.push(key);
                }
            }
            let mut steps = 0;
            while elapsed >= DT && steps < MAX_STEPS_PER_FRAME {
                let keys = Keys {
                    held: held(),
                    pressed: std::mem::take(&mut pending),
                };
                if let Err(error) = game.step(&keys, DT) {
                    eprintln!("{error}");
                    failure = Some(error);
                    break;
                }
                elapsed -= DT;
                steps += 1;
            }
            if steps == MAX_STEPS_PER_FRAME {
                elapsed = 0.0;
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

fn held() -> Vec<&'static str> {
    KEYS.iter()
        .filter(|(code, _)| is_key_down(*code))
        .map(|(_, name)| *name)
        .collect()
}

fn just_pressed() -> impl Iterator<Item = &'static str> {
    KEYS.iter()
        .filter(|(code, _)| is_key_pressed(*code))
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
