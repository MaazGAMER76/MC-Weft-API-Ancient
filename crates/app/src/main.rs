use std::path::PathBuf;

use weft_engine::Options;
use weft_engine::paths::{self, Dirs};
use weft_render::{Backend, BackendDesc, Error, Renderer};

#[global_allocator]
static ALLOC: weft_engine::diag::CountingAlloc = weft_engine::diag::CountingAlloc;

fn backend(renderer: Renderer, desc: &BackendDesc) -> Result<Box<dyn Backend>, Error> {
    match renderer {
        Renderer::Vulkan => {
            #[cfg(feature = "renderer-vulkan")]
            {
                Ok(Box::new(weft_vulkan::VulkanBackend::new(desc)?))
            }
            #[cfg(not(feature = "renderer-vulkan"))]
            {
                Err(Error("Vulkan renderer was not enabled at build time".into()))
            }
        }
        Renderer::OpenGl21 => {
            #[cfg(feature = "renderer-gl21")]
            {
                Ok(Box::new(weft_backend_opengl21::Gl21Backend::new(desc)?))
            }
            #[cfg(not(feature = "renderer-gl21"))]
            {
                Err(Error("OpenGL 2.1 renderer was not enabled at build time".into()))
            }
        }
        Renderer::DirectX10 => {
            #[cfg(feature = "renderer-d3d10")]
            {
                Ok(Box::new(weft_backend_d3d10::D3d10Backend::new(desc)?))
            }
            #[cfg(not(feature = "renderer-d3d10"))]
            {
                Err(Error("Direct3D 10 renderer was not enabled at build time".into()))
            }
        }
        Renderer::Cpu => {
            #[cfg(feature = "renderer-cpu")]
            {
                Ok(Box::new(weft_backend_cpu::CpuBackend::new(desc)?))
            }
            #[cfg(not(feature = "renderer-cpu"))]
            {
                Err(Error("CPU renderer was not enabled at build time".into()))
            }
        }
        Renderer::DirectX12 => Err(Error("Direct3D 12 renderer is not implemented".into())),
    }
}

/// Where `run` (and, in the source tree, `mods`) live: the working directory if it has either,
/// otherwise the nearest folder above the executable that does (a development build runs from
/// `target/<profile>/`).
fn game_root() -> PathBuf {
    let has = |d: &std::path::Path| d.join("run").is_dir() || d.join("mods").is_dir();
    let cwd = std::env::current_dir().unwrap_or_default();
    if has(&cwd) {
        return cwd;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.ancestors()
        .skip(1)
        .find(|d| has(d))
        .map(PathBuf::from)
        .unwrap_or(cwd)
}

fn main() {
    let root = game_root();
    let mut opts = Options {
        run_dir: PathBuf::new(),
        cache_dir: PathBuf::new(),
        // Mod sources of the game itself; players' mods go into `<run dir>/mods`, added below.
        mod_dirs: vec![root.join("mods")],
        gpu_debug: false,
        world: None,
        seed: None,
        screen: None,
        size: (1600, 900),
        dev: cfg!(debug_assertions),
        automated: false,
        no_vsync: false,
        view: None,
        renderer: None,
        render_select: false,
        renderer_config: PathBuf::new(),
    };
    let mut chosen_run: Option<PathBuf> = None;
    let mut verbose = false;
    let mut log_spec = String::new();
    let mut server = false;
    let mut pack: Option<PathBuf> = None;
    let mut dist: Option<PathBuf> = None;
    // Options the engine does not know go to mods (`--test furnace`, `--script x.txt`).
    let mut extra: Vec<(String, String)> = Vec::new();
    let mut args = std::env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_default();
        match a.as_str() {
            "--run-dir" => chosen_run = Some(PathBuf::from(value())),
            "--mods" => opts.mod_dirs.push(PathBuf::from(value())),
            "--gpu-debug" => opts.gpu_debug = true,
            "--verbose" => verbose = true,
            "--log" => log_spec = value(),
            "--world" => opts.world = Some(value()),
            "--seed" => opts.seed = value().parse().ok(),
            "--screen" => opts.screen = Some(value()),
            "--render" => {
                let selected = value();
                opts.renderer = Renderer::parse(&selected);
                if opts.renderer.is_none() {
                    eprintln!("unknown renderer {selected}; expected vulkan, dx12, d3d10, gl21, or cpu");
                    std::process::exit(2);
                }
            }
            "--render-select" => opts.render_select = true,
            "--no-vsync" => opts.no_vsync = true,
            "--server" => server = true,
            // Engine shaders and assets live from the source tree in a release build too.
            "--dev" => opts.dev = true,
            "--view" => opts.view = value().parse().ok(),
            "--pack-mod" => pack = Some(PathBuf::from(value())),
            "--dist" => dist = Some(PathBuf::from(value())),
            "--size" => {
                let v: Vec<u32> = value().split('x').filter_map(|s| s.parse().ok()).collect();
                if v.len() == 2 {
                    opts.size = (v[0], v[1]);
                }
            }
            other => match other.strip_prefix("--") {
                Some(name) => {
                    let v = if args.peek().is_some_and(|n| !n.starts_with("--")) { args.next().unwrap_or_default() } else { "1".into() };
                    extra.push((name.to_string(), v));
                }
                None => eprintln!("unknown argument {other}"),
            },
        }
    }
    let dirs = chosen_run.map_or_else(|| paths::of_install(&root), |run| Dirs::under(&run));
    (opts.run_dir, opts.cache_dir) = (dirs.data, dirs.cache);
    opts.renderer_config = dirs.config.join("render.toml");
    if opts.renderer.is_none() {
        opts.renderer = read_renderer_config(&opts.renderer_config).filter(|renderer| matches!(renderer, Renderer::Vulkan | Renderer::Cpu));
        if !server && (opts.render_select || opts.renderer.is_none()) {
            opts.render_select = true;
        }
    } else {
        opts.render_select = false;
    }
    paths::set_config(dirs.config);
    opts.automated = extra.iter().any(|(k, _)| k == "script" || k == "test");
    if let Some(code) = test_jobs(&extra, &opts.run_dir) {
        std::process::exit(code);
    }
    weft_engine::set_launch_options(extra);
    let player_mods = opts.run_dir.join("mods");
    if let Some(dir) = pack {
        match weft_engine::modloader::pack(&dir, &player_mods) {
            Ok(path) => println!("packed {}", path.display()),
            Err(e) => eprintln!("cannot pack {}: {e}", dir.display()),
        }
        return;
    }
    if let Some(out) = dist {
        match make_dist(&root, &out) {
            Ok(()) => println!("game ready in {}", out.display()),
            Err(e) => eprintln!("cannot make {}: {e}", out.display()),
        }
        return;
    }
    weft_engine::log::init(Some(&opts.run_dir.join("logs").join("latest.log")), verbose, &log_spec);
    weft_engine::diag::install_crash_handler(&opts.run_dir);
    log::info!("game folder {}, cache {}", opts.run_dir.display(), opts.cache_dir.display());
    // Release builds load the game's own mods the way players get them: each packed into one
    // file in `run/mods` (again only when it changed).
    if !cfg!(debug_assertions) && root.join("mods").is_dir() {
        for dir in subdirs(&root.join("mods")) {
            match weft_engine::modloader::pack_if_stale(&dir, &player_mods) {
                Ok((path, true)) => log::info!("packed {}", path.display()),
                Ok(_) => {}
                Err(e) => log::warn!("cannot pack {}: {e}", dir.display()),
            }
        }
        let sources = root.join("mods");
        opts.mod_dirs.retain(|d| *d != sources);
    }
    // What came with a shipped game stays with it, and a player's copy of a mod comes first.
    let shipped = root.join("run").join("mods");
    let separate = shipped != player_mods;
    opts.mod_dirs.push(player_mods);
    if separate {
        opts.mod_dirs.push(shipped);
    }
    weft_engine::init_workers();
    if server {
        if let Err(e) = weft_engine::dedicated::run(opts) {
            log::error!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(e) = weft_engine::run(backend, opts) {
        log::error!("{e}");
        std::process::exit(1);
    }
    std::process::exit(weft_engine::exit_code());
}

fn read_renderer_config(path: &std::path::Path) -> Option<Renderer> {
    let text = std::fs::read_to_string(path).ok()?;
    let config: toml::Value = toml::from_str(&text).ok()?;
    Renderer::parse(config.get("renderer")?.as_str()?)
}

/// `--test all` runs in several processes at once, each with its own run folder (`--jobs N`, 1 for
/// one after the other, also what naming tests does unless asked). Each takes every Nth test
/// (`testkit`, `--test-shard`); what they leave is put together as a single run would have it.
fn test_jobs(extra: &[(String, String)], run_dir: &std::path::Path) -> Option<i32> {
    let option = |name: &str| {
        extra
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    if option("test-shard").is_some() {
        return None;
    }
    let test = option("test")?;
    let auto = (std::thread::available_parallelism().map_or(1, |n| n.get()) / 4).clamp(1, 6);
    let jobs = option("jobs")
        .and_then(|j| j.parse().ok())
        .unwrap_or(if test == "all" { auto } else { 1 });
    if jobs < 2 || test == "list" {
        return None;
    }
    let started = std::time::Instant::now();
    let exe = std::env::current_exe().ok()?;
    let mut dirs: Vec<PathBuf> = (0..jobs)
        .map(|i| run_dir.join("test-jobs").join(i.to_string()))
        .collect();
    let spawn = |shard: &str, dir: &PathBuf| {
        let _ = std::fs::create_dir_all(dir);
        // Or a process that dies at once passes by the last run's.
        let _ = std::fs::remove_file(dir.join("test-reports").join("report.txt"));
        let log = std::fs::File::create(dir.join("output.log")).ok()?;
        let mut c = std::process::Command::new(&exe);
        c.args(["--test", test, "--test-shard", shard, "--run-dir"])
            .arg(dir)
            .stdout(log)
            .stderr(std::process::Stdio::null());
        for (k, v) in extra.iter().filter(|(k, _)| k != "test" && k != "jobs") {
            c.arg(format!("--{k}"));
            if !v.is_empty() {
                c.arg(v);
            }
        }
        c.spawn().ok()
    };
    let children: Vec<_> = dirs
        .iter()
        .enumerate()
        .map(|(i, dir)| spawn(&format!("{i}/{jobs}"), dir))
        .collect();
    let code = |c: Option<std::process::Child>| {
        c.and_then(|mut c| c.wait().ok())
            .and_then(|s| s.code())
            .unwrap_or(1)
    };
    let mut codes: Vec<i32> = children.into_iter().map(code).collect();
    // What measures how quick the game is can't share the machine: last, on its own.
    dirs.push(run_dir.join("test-jobs").join("alone"));
    codes.push(code(spawn("alone", &dirs[jobs])));
    // A crashed process has no report, and its tests didn't pass.
    let shots = run_dir.join("test-shots");
    let _ = std::fs::remove_dir_all(&shots);
    let _ = std::fs::create_dir_all(&shots);
    let (mut passed, mut failed, mut details, mut crashed) = (0, 0, String::new(), 0);
    for (i, dir) in dirs.iter().enumerate() {
        for e in std::fs::read_dir(dir.join("test-shots"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let _ = std::fs::rename(e.path(), shots.join(e.file_name()));
        }
        let report = std::fs::read_to_string(dir.join("test-reports/report.txt")).unwrap_or_default();
        let counts: Vec<i32> = report
            .lines()
            .next()
            .unwrap_or("")
            .split_whitespace()
            .filter_map(|w| w.parse().ok())
            .collect();
        if let ([p, f], true) = (counts.as_slice(), codes[i] >= 0 && !report.is_empty()) {
            (passed, failed) = (passed + p, failed + f);
            details.push_str(report.split_once('\n').map_or("", |r| r.1));
        } else {
            crashed += 1;
            details.push_str(&format!("\nFAIL process {i} ended without a report, see {}\n", dir.join("output.log").display()));
        }
    }
    let head = format!("testkit report: {passed} passed, {failed} failed, {:.1} s, {jobs} at once\n", started.elapsed().as_secs_f32());
    let reports = run_dir.join("test-reports");
    let _ = std::fs::create_dir_all(&reports);
    let _ = std::fs::write(reports.join("report.txt"), format!("{head}{details}"));
    print!("{head}");
    for l in details
        .lines()
        .scan(false, |in_fail, l| {
            *in_fail = if l.starts_with("FAIL") {
                true
            } else if l.starts_with("PASS") {
                false
            } else {
                *in_fail
            };
            Some(in_fail.then_some(l))
        })
        .flatten()
    {
        println!("{l}");
    }
    Some(failed + crashed)
}

fn subdirs(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// A folder to hand to someone: the executable, the engine's assets and every mod packed.
fn make_dist(root: &std::path::Path, out: &std::path::Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    std::fs::copy(&exe, out.join(exe.file_name().unwrap())).map_err(|e| e.to_string())?;
    copy_dir(&root.join("crates").join("engine").join("assets"), &out.join("assets").join("core"))?;
    for dir in subdirs(&root.join("mods")) {
        // Development tools (the test kit) mark themselves `dist = false` in Cargo.toml.
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
        if manifest.lines().any(|l| l.replace(' ', "") == "dist=false") {
            continue;
        }
        weft_engine::modloader::pack(&dir, &out.join("run").join("mods"))?;
    }
    Ok(())
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(from)
        .map_err(|e| format!("{}: {e}", from.display()))?
        .flatten()
    {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&p, to.join(e.file_name())).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
