//! Compilation-only measurements; no generated NC program is ever executed.
use std::{
    env,
    fmt::Write as _,
    fs,
    hint::black_box,
    path::Path,
    process::{Command, ExitCode},
    time::{Duration, Instant},
};

enum Workload {
    ExpressionList,
    SharedTypeGraph,
}

impl Workload {
    fn name(&self) -> &'static str {
        match self {
            Self::ExpressionList => "expression_list",
            Self::SharedTypeGraph => "shared_type_graph",
        }
    }

    fn source(&self, size: usize) -> String {
        match self {
            Self::ExpressionList => expression_list(size),
            Self::SharedTypeGraph => shared_type_graph(size),
        }
    }

    fn compile(&self, source: &str, release: bool) -> Result<String, String> {
        let path = Path::new("compilation-benchmark.nc");
        let result = match self {
            Self::ExpressionList => ncc::compile_test_source_with_options(source, path, release),
            Self::SharedTypeGraph => ncc::compile_source_with_options(source, path, release),
        };
        result.map_err(|error| error.render(source, path))
    }
}

struct Options {
    samples: usize,
    warmup: usize,
    sizes: Vec<usize>,
    depths: Vec<usize>,
    native: bool,
}

fn parse_number(value: &str, positive: bool) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|number| !positive || *number > 0)
        .ok_or_else(|| {
            format!(
                "invalid {}integer `{value}`",
                if positive { "positive " } else { "" }
            )
        })
}

fn parse_sizes(value: &str) -> Result<Vec<usize>, String> {
    let sizes = value
        .split(',')
        .map(|size| parse_number(size, true))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, size) in sizes.iter().enumerate() {
        if sizes[..index].contains(size) {
            return Err(format!("duplicate size `{size}`"));
        }
    }
    Ok(sizes)
}

fn options() -> Result<Option<Options>, String> {
    let mut options = Options {
        samples: 7,
        warmup: 2,
        sizes: vec![128, 512, 2048],
        depths: vec![8, 16, 24],
        native: false,
    };
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Compilation-only benchmark\n\n--samples N       Measured samples (default 7)\n--warmup N        Untimed warmups (default 2)\n--sizes N,N       Expression counts (default 128,512,2048)\n--depths N,N      Shared DAG depths (default 8,16,24)\n--native          Separately measure cc compilation/linking\n\nCSV is written to stdout; no NC program is executed."
                );
                return Ok(None);
            }
            // Cargo appends this flag even for a custom benchmark harness.
            "--bench" => {}
            "--native" => options.native = true,
            "--samples" | "--warmup" | "--sizes" | "--depths" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("{arg} requires a value"))?;
                match arg.as_str() {
                    "--samples" => options.samples = parse_number(&value, true)?,
                    "--warmup" => options.warmup = parse_number(&value, false)?,
                    "--sizes" => options.sizes = parse_sizes(&value)?,
                    _ => options.depths = parse_sizes(&value)?,
                }
            }
            _ => return Err(format!("unknown option `{arg}`")),
        }
    }
    Ok(Some(options))
}

fn expression_list(size: usize) -> String {
    let mut source = String::from(
        "(float, float) pair = (0.25, 2.0)\n\
         fn scale = fn(float value) float { return (value + pair[0]) * pair[1] }\n\
         test \"independent expressions\" {\n\
         assert @args().len > 0\n",
    );
    // Unknown input prevents whole-program replacement, retaining independent
    // folding attempts and immutable tuple/by-value capture evaluation.
    for value in 0..size {
        let expected = 2 * value;
        writeln!(
            source,
            "assert ({value}.0 + 0.25) * 2.0 == {expected}.5\nassert scale({value}.0) == {expected}.5"
        )
        .unwrap();
    }
    source.push_str("}\n");
    source
}

fn shared_type_graph(depth: usize) -> String {
    let mut source = String::from("type A0 = int\nstruct S0 { int value }\n");
    for level in 1..=depth {
        let previous = level - 1;
        writeln!(source, "type A{level} = (A{previous}, A{previous})").unwrap();
        writeln!(
            source,
            "struct S{level} {{ S{previous} left S{previous} right }}"
        )
        .unwrap();
    }
    source
}

fn native_compile(c: &str, path: &Path, output: &Path, release: bool) -> Result<Duration, String> {
    let mut command = Command::new("cc");
    command.arg(path).arg("-std=c11");
    if cfg!(target_os = "macos") {
        command.args(["-arch", ncc::target::ARCH]);
    }
    if release {
        command.arg("-O3");
    } else {
        command.args(["-O0", "-g"]);
    }
    if c.contains("#include <pthread.h>") {
        command.arg("-pthread");
    }
    if c.contains("#include <math.h>") {
        command.arg("-lm");
    }
    command.arg("-o").arg(output);
    let start = Instant::now();
    let result = command
        .output()
        .map_err(|error| format!("cannot run cc: {error}"))?;
    let elapsed = start.elapsed();
    if !result.status.success() {
        return Err(format!(
            "cc failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(elapsed)
}

fn record(
    workload: &Workload,
    size: usize,
    release: bool,
    phase: &str,
    sample: usize,
    elapsed: Duration,
    bytes: (usize, usize),
) {
    let name = workload.name();
    let mode = if release { "release" } else { "debug" };
    println!(
        "{name},{size},{mode},{phase},{sample},{},{},{}",
        elapsed.as_nanos(),
        bytes.0,
        bytes.1
    );
}

fn measure(workload: &Workload, size: usize, options: &Options) -> Result<(), String> {
    // Fixture creation, temporary files, CSV output and result destruction are
    // outside the NC timer. Each compile gets fresh compiler/evaluator state.
    let source = workload.source(size);
    let directory = ncc::temp::Directory::new().map_err(|error| error.to_string())?;
    for release in [false, true] {
        let mut c = String::new();
        for _ in 0..options.warmup {
            black_box(workload.compile(black_box(&source), release)?);
        }
        for sample in 0..options.samples {
            let start = Instant::now();
            let compiled = workload.compile(black_box(&source), release)?;
            let elapsed = start.elapsed();
            record(
                workload,
                size,
                release,
                "nc_compile",
                sample,
                elapsed,
                (source.len(), compiled.len()),
            );
            c = black_box(compiled);
        }
        if options.native {
            let input = directory.path().join("program.c");
            let output = directory.path().join("program");
            fs::write(&input, &c).map_err(|error| error.to_string())?;
            for _ in 0..options.warmup {
                native_compile(&c, &input, &output, release)?;
            }
            for sample in 0..options.samples {
                let elapsed = native_compile(&c, &input, &output, release)?;
                record(
                    workload,
                    size,
                    release,
                    "native_c",
                    sample,
                    elapsed,
                    (source.len(), c.len()),
                );
            }
        }
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let Some(options) = options()? else {
        return Ok(());
    };
    println!("workload,size,mode,phase,sample,elapsed_ns,source_bytes,c_bytes");
    for size in &options.sizes {
        measure(&Workload::ExpressionList, *size, &options)?;
    }
    for depth in &options.depths {
        measure(&Workload::SharedTypeGraph, *depth, &options)?;
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("compilation benchmark: {error}");
            ExitCode::FAILURE
        }
    }
}
