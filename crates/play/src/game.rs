use std::rc::Rc;

use ludo_core::checker::Checker;
use ludo_core::compiler::compile;
use ludo_core::interpreter::RuntimeError;
use ludo_core::parser::parse;
use ludo_core::scanner::scan_tokens;
use ludo_core::value::{Instance, Value};
use ludo_core::vm::Vm;

/// What a game can use without declaring it: the input, the shapes it
/// draws, the colors and the config. The CLI loads it too, so a game file
/// runs and checks the same with `ludo run`.
pub const HOST: &str = include_str!("host.ludo");

const CONTRACT: [(&str, usize, &str); 3] = [
    ("init", 0, "init :: () -> World"),
    (
        "update",
        3,
        "update :: (w: World, input: Input, dt: Float) -> World",
    ),
    ("draw", 1, "draw :: (w: World) -> List[Shape]"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub width: i32,
    pub height: i32,
    pub title: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            width: 640,
            height: 480,
            title: "ludo".to_string(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Keys {
    pub held: Vec<&'static str>,
    pub pressed: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Rgba,
    },
    Circle {
        x: f32,
        y: f32,
        r: f32,
        color: Rgba,
    },
    Line {
        from: (f32, f32),
        to: (f32, f32),
        thickness: f32,
        color: Rgba,
    },
    Text {
        x: f32,
        y: f32,
        size: f32,
        text: String,
        color: Rgba,
    },
}

/// A game running on the VM: its code is compiled to bytecode once, when it
/// loads or reloads.
pub struct Game {
    vm: Vm,
    world: Value,
    config: Config,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reload {
    Kept,
    Restarted,
}

impl Game {
    pub fn load(source: &str) -> Result<Game, Vec<String>> {
        let (mut vm, config) = prepare(source)?;
        let world = init(&mut vm).map_err(|e| vec![e])?;
        Ok(Game { vm, world, config })
    }

    pub fn reload(&mut self, source: &str) -> Result<Reload, Vec<String>> {
        let (mut vm, _) = prepare(source)?;
        let before = self.vm.struct_shapes();
        let after = vm.struct_shapes();
        let changed = before
            .iter()
            .any(|(name, fields)| after.get(name) != Some(fields));

        if changed {
            self.world = init(&mut vm).map_err(|e| vec![e])?;
            self.vm = vm;
            return Ok(Reload::Restarted);
        }
        vm.adopt_sequences(&mut self.vm);
        self.vm = vm;
        Ok(Reload::Kept)
    }

    pub fn restart(&mut self) -> Result<(), String> {
        self.vm.stop_sequences();
        self.world = init(&mut self.vm)?;
        Ok(())
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn step(&mut self, keys: &Keys, dt: f64) -> Result<(), String> {
        let args = vec![self.world.clone(), input(keys), Value::Float(dt)];
        let world = self
            .vm
            .call_function("update", args)
            .map_err(|e| runtime("update", e))?;
        self.world = self
            .vm
            .run_sequences(world, dt)
            .map_err(|e| runtime("a sequence", e))?;
        Ok(())
    }

    pub fn draw(&mut self) -> Result<Vec<Shape>, String> {
        let shapes = self
            .vm
            .call_function("draw", vec![self.world.clone()])
            .map_err(|e| runtime("draw", e))?;
        let Value::List(items) = shapes else {
            return Err(format!(
                "`draw` must return a List of Rect, Circle, Line and Text, got {}",
                shapes.type_name()
            ));
        };
        items.iter().map(shape).collect()
    }
}

fn prepare(source: &str) -> Result<(Vm, Config), Vec<String>> {
    let mut vm = Vm::new();
    let mut checker = Checker::new();
    run(&mut vm, &mut checker, HOST).expect("the host prelude is valid");
    run(&mut vm, &mut checker, source)?;

    let mut errors = Vec::new();
    for (name, arity, signature) in CONTRACT {
        match vm.arity(name) {
            Some(n) if n == arity => {}
            Some(n) => errors.push(format!(
                "`{name}` takes {n} parameters, but a game needs `{signature}`"
            )),
            None => errors.push(format!("a game needs `{signature}`")),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let config = match vm.arity("config") {
        None => Config::default(),
        Some(0) => {
            let value = vm
                .call_function("config", vec![])
                .map_err(|e| vec![runtime("config", e)])?;
            read_config(&value).map_err(|e| vec![e])?
        }
        Some(n) => {
            return Err(vec![format!(
                "`config` takes {n} parameters, but it must be `config :: () -> Config`"
            )])
        }
    };
    Ok((vm, config))
}

fn init(vm: &mut Vm) -> Result<Value, String> {
    vm.call_function("init", vec![])
        .map_err(|e| runtime("init", e))
}

/// Checks the names of a program, after what `checker` has seen, and runs it.
fn run(vm: &mut Vm, checker: &mut Checker, source: &str) -> Result<(), Vec<String>> {
    let tokens = scan_tokens(source).map_err(|errors| {
        errors
            .iter()
            .map(|e| format!("[line {}] Scanning Error: {}", e.line, e.message))
            .collect::<Vec<_>>()
    })?;
    let statements = parse(tokens).map_err(|errors| {
        errors
            .iter()
            .map(|e| format!("[line {}] Parsing Error: {}", e.line, e.message))
            .collect::<Vec<_>>()
    })?;
    checker
        .check(&statements)
        .map_err(|errors| errors.iter().map(|e| e.to_string()).collect::<Vec<_>>())?;
    for stmt in &statements {
        let script = compile(stmt).map_err(|e| vec![e.to_string()])?;
        vm.run(script)
            .map_err(|e| vec![runtime("the program", e)])?;
    }
    Ok(())
}

fn runtime(during: &str, error: RuntimeError) -> String {
    match error.line {
        Some(line) => format!("[line {line}] Runtime Error in {during}: {}", error.message),
        None => format!("Runtime Error in {during}: {}", error.message),
    }
}

fn input(keys: &Keys) -> Value {
    let symbols = |names: &[&str]| {
        let symbols = names.iter().map(|name| Value::Symbol(name.to_string()));
        Value::List(Rc::new(symbols.collect()))
    };
    let mut fields = vec![
        ("held".to_string(), symbols(&keys.held)),
        ("pressed".to_string(), symbols(&keys.pressed)),
    ];
    for name in ["up", "down", "left", "right", "w", "a", "s", "d", "space"] {
        fields.push((name.to_string(), Value::Bool(keys.held.contains(&name))));
    }
    Value::Struct(Rc::new(Instance {
        name: "Input".to_string(),
        fields,
    }))
}

fn shape(value: &Value) -> Result<Shape, String> {
    let Value::Struct(instance) = value else {
        return Err(format!(
            "`draw` must return a List of Rect, Circle, Line and Text, got a {} in it",
            value.type_name()
        ));
    };
    let float = |field: &str| number(instance, field);
    match instance.name.as_str() {
        "Rect" => Ok(Shape::Rect {
            x: float("x")?,
            y: float("y")?,
            w: float("w")?,
            h: float("h")?,
            color: color(instance)?,
        }),
        "Circle" => Ok(Shape::Circle {
            x: float("x")?,
            y: float("y")?,
            r: float("r")?,
            color: color(instance)?,
        }),
        "Line" => Ok(Shape::Line {
            from: (float("x1")?, float("y1")?),
            to: (float("x2")?, float("y2")?),
            thickness: float("thickness")?,
            color: color(instance)?,
        }),
        "Text" => Ok(Shape::Text {
            x: float("x")?,
            y: float("y")?,
            size: float("size")?,
            text: match instance.get("text") {
                Some(Value::Str(text)) => text.clone(),
                other => return Err(field_error(instance, "text", "a String", other)),
            },
            color: color(instance)?,
        }),
        other => Err(format!(
            "`draw` must return a List of Rect, Circle, Line and Text, got a {other} in it"
        )),
    }
}

fn read_config(value: &Value) -> Result<Config, String> {
    let Value::Struct(config) = value else {
        return Err(format!(
            "`config` must return a Config, got {}",
            value.type_name()
        ));
    };
    if config.name != "Config" {
        return Err(format!(
            "`config` must return a Config, got {}",
            config.name
        ));
    }
    let size = |field: &str| match config.get(field) {
        Some(Value::Int(v)) if (1..=4096).contains(v) => Ok(*v as i32),
        Some(Value::Int(v)) => Err(format!(
            "`Config.{field}` must be between 1 and 4096, got {v}"
        )),
        other => Err(field_error(config, field, "an Int", other)),
    };
    let title = match config.get("title") {
        Some(Value::Str(title)) => title.clone(),
        other => return Err(field_error(config, "title", "a String", other)),
    };
    Ok(Config {
        width: size("width")?,
        height: size("height")?,
        title,
    })
}

fn number(instance: &Instance, field: &str) -> Result<f32, String> {
    match instance.get(field) {
        Some(Value::Float(v)) => Ok(*v as f32),
        other => Err(field_error(instance, field, "a Float", other)),
    }
}

fn color(instance: &Instance) -> Result<Rgba, String> {
    match instance.get("color") {
        Some(Value::Symbol(name)) => named(name),
        Some(Value::Struct(color)) if color.name == "Color" => {
            let channel = |field: &str| match color.get(field) {
                Some(Value::Int(v)) if (0..=255).contains(v) => Ok(*v as u8),
                Some(Value::Int(v)) => Err(format!(
                    "`Color.{field}` must be between 0 and 255, got {v}"
                )),
                other => Err(field_error(color, field, "an Int", other)),
            };
            Ok(Rgba(
                channel("r")?,
                channel("g")?,
                channel("b")?,
                channel("a")?,
            ))
        }
        other => Err(field_error(instance, "color", "a Color or a Symbol", other)),
    }
}

fn named(name: &str) -> Result<Rgba, String> {
    let (r, g, b) = match name {
        "white" => (255, 255, 255),
        "black" => (0, 0, 0),
        "gray" => (128, 128, 128),
        "red" => (230, 41, 55),
        "green" => (0, 228, 48),
        "blue" => (0, 121, 241),
        "yellow" => (253, 249, 0),
        "orange" => (255, 161, 0),
        "purple" => (200, 122, 255),
        "pink" => (255, 109, 194),
        other => {
            return Err(format!(
                "unknown color `:{other}`: use rgb(r, g, b) or one of :white, :black, :gray, :red, :green, :blue, :yellow, :orange, :purple, :pink"
            ))
        }
    };
    Ok(Rgba(r, g, b, 255))
}

fn field_error(instance: &Instance, field: &str, expected: &str, got: Option<&Value>) -> String {
    match got {
        Some(value) => format!(
            "`{}.{field}` must be {expected}, got {}",
            instance.name,
            value.type_name()
        ),
        None => format!("`{}` has no field `{field}`", instance.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: &str = include_str!("../../../examples/square.ludo");
    const PONG: &str = include_str!("../../../examples/pong.ludo");
    const FRAME: f64 = 1.0 / 60.0;

    fn get<'a>(world: &'a Value, path: &[&str]) -> &'a Value {
        path.iter().fold(world, |value, field| match value {
            Value::Struct(instance) => instance.get(field).expect("the field exists"),
            other => panic!("{other} has no fields"),
        })
    }

    fn steps(game: &mut Game, count: usize) {
        for _ in 0..count {
            game.step(&Keys::default(), FRAME).unwrap();
        }
    }

    #[test]
    fn plays_the_square() {
        let mut game = Game::load(SQUARE).unwrap();
        let right = Keys {
            held: vec!["right"],
            pressed: vec![],
        };
        for _ in 0..4 {
            game.step(&right, 0.25).unwrap();
        }
        assert_eq!(
            game.draw().unwrap(),
            [Shape::Rect {
                x: 520.0,
                y: 240.0,
                w: 20.0,
                h: 20.0,
                color: Rgba(253, 249, 0, 255),
            }]
        );
    }

    #[test]
    fn pong_serves_through_a_sequence() {
        let mut game = Game::load(PONG).unwrap();
        assert_eq!(game.config().title, "Pong");
        steps(&mut game, 30);
        assert_eq!(get(&game.world, &["serving"]), &Value::Bool(true));
        // `serve` waits 0.6s and grows the ball for 0.4s, then sends it left.
        steps(&mut game, 40);
        assert_eq!(get(&game.world, &["serving"]), &Value::Bool(false));
        assert_eq!(get(&game.world, &["ball", "vx"]), &Value::Float(-320.0));
        // The 16 dashes of the net, the walls, the paddles, the ball and the
        // two scores.
        assert_eq!(game.draw().unwrap().len(), 23);
    }

    #[test]
    fn a_reload_keeps_the_world_unless_a_struct_changes() {
        let mut game = Game::load(PONG).unwrap();
        steps(&mut game, 30);
        assert_eq!(game.reload(PONG).unwrap(), Reload::Kept);
        // The serve that was running carries over and finishes.
        steps(&mut game, 40);
        assert_eq!(get(&game.world, &["serving"]), &Value::Bool(false));

        let tiny = |fields: &str, value: &str| {
            format!(
                "World :: struct {{ {fields} }}\n\
                 init :: () -> World {{ World {{ {value} }} }}\n\
                 update :: (w: World, input: Input, dt: Float) -> World {{ w }}\n\
                 draw :: (w: World) -> List[Shape] {{ [] }}"
            )
        };
        let mut game = Game::load(&tiny("x: Float", "x: 1.0")).unwrap();
        let reload = game.reload(&tiny("x: Float, y: Float", "x: 2.0, y: 0.0"));
        assert_eq!(reload.unwrap(), Reload::Restarted);
        assert_eq!(get(&game.world, &["x"]), &Value::Float(2.0));
    }

    #[test]
    fn a_game_needs_its_functions() {
        let errors = Game::load("init :: () -> Int { 1 }").err().unwrap();
        assert_eq!(
            errors,
            [
                "a game needs `update :: (w: World, input: Input, dt: Float) -> World`",
                "a game needs `draw :: (w: World) -> List[Shape]`",
            ]
        );
    }
}
