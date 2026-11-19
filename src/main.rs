mod floatkb;
mod floatwindow;
mod hypr;
mod instance;
mod keyboard;
mod shell;
mod theme;

use anyhow::{Context, Result, bail};
use gpui_kit::{App, WindowingModes};
use instance::{Command, Instance};
use std::{cell::RefCell, env, path::Path, rc::Rc};

fn main() -> Result<()> {
    let mut args = env::args();
    let invoked = args.next().unwrap_or_default();
    let name = Path::new(&invoked)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let mut args: Vec<_> = args.collect();
    let subcommand;
    let name = if matches!(name, "floatkb" | "floatwindow") {
        name
    } else if args
        .first()
        .is_some_and(|a| matches!(a.as_str(), "floatkb" | "floatwindow"))
    {
        subcommand = args.remove(0);
        subcommand.as_str()
    } else {
        bail!("usage: noctalia-touch <floatkb|floatwindow> [options]");
    };
    if args.as_slice() == ["--help"] {
        println!(
            "usage: {name}{}",
            if name == "floatkb" {
                " [--toggle|--collapsed]"
            } else {
                ""
            }
        );
        return Ok(());
    }
    let (command, collapsed) = match (name, args.as_slice()) {
        ("floatkb", []) => (Command::Expand, false),
        ("floatkb", [a]) if a == "--toggle" => (Command::Toggle, false),
        ("floatkb", [a]) if a == "--collapsed" => (Command::Expand, true),
        ("floatwindow", []) => (Command::Close, false),
        _ => bail!("{name}: unknown arguments"),
    };
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .context("XDG_RUNTIME_DIR is not set; run in a Wayland session")?;
    let Some(owner) = Instance::acquire(Path::new(&runtime), name, command)? else {
        return Ok(());
    };
    let commands = owner.listen()?;
    let result = Rc::new(RefCell::new(Ok(())));
    let status = result.clone();
    let keyboard = name == "floatkb";
    gpui_kit::platform::linux(WindowingModes::WAYLAND).run(move |cx: &mut App| {
        gpui_kit::init(cx);
        gpui_kit_material::init(cx);
        let setup = if keyboard {
            floatkb::run(cx, collapsed, commands)
        } else {
            cx.spawn(async move |cx| {
                if commands.recv().await.is_ok() {
                    cx.update(|cx| cx.quit());
                }
            })
            .detach();
            floatwindow::run(cx)
        };
        if let Err(e) = setup {
            *status.borrow_mut() = Err(e);
            cx.quit();
        }
    });
    result.replace(Ok(()))
}
