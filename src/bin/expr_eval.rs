//! Protocol-compatible replacement for slosh/tests/expr_eval.c.
use slush::expr::{Environment, Program};
use std::io::{self, BufRead};

fn main() {
    for line in io::stdin().lock().lines() {
        let line = line.expect("read expression request");
        if line.is_empty() {
            continue;
        }
        let Some((numbers, source)) = line.split_once('\t') else {
            println!("error: no expression");
            continue;
        };
        let values: Result<Vec<i64>, _> = numbers.split_whitespace().map(str::parse).collect();
        let Ok(values) = values else {
            println!("error: bad environment");
            continue;
        };
        if values.len() != 10 {
            println!("error: bad environment");
            continue;
        }
        let env = Environment {
            x: values[0] as i32,
            y: values[1] as i32,
            cols: values[2] as i32,
            rows: values[3] as i32,
            curx: values[4] as i32,
            cury: values[5] as i32,
            cursor: values[6] as i32,
            focused: values[7] as i32,
            t: values[8],
            since: values[9],
            ..Default::default()
        };
        match Program::compile(source) {
            Ok(program) => println!("{}", program.eval(&env)),
            Err(error) => println!("error: {error}"),
        }
    }
}
