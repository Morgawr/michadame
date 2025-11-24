use anyhow::{anyhow, Context, Result};
use libpulse_binding::callbacks::ListResult;
use libpulse_binding::context::{Context as PulseContext, FlagSet as PulseContextFlagSet, State as PulseContextState};
use libpulse_binding::def::Retval;
use libpulse_binding::mainloop::standard::{IterateResult, Mainloop};
use libpulse_binding::operation::State as OperationState;
use std::cell::RefCell;
use std::rc::Rc;

fn run_pulse_op<F, T>(op_logic: F) -> Result<T>
where
    F: FnOnce(&mut PulseContext, &mut Mainloop) -> Result<T>,
{
    let mut mainloop = Mainloop::new().context("Failed to create mainloop")?;
    let mut context = PulseContext::new(&mainloop, "pa-client").context("Failed to create context")?;

    context.connect(None, PulseContextFlagSet::empty(), None).context("Failed to connect context")?;

    loop {
        match mainloop.iterate(false) {
            IterateResult::Err(e) => return Err(anyhow!("Mainloop iterate error: {}", e)),
            IterateResult::Quit(_) => return Err(anyhow!("Mainloop quit unexpectedly")),
            _ => {}
        }
        match context.get_state() {
            PulseContextState::Ready => break,
            PulseContextState::Failed | PulseContextState::Terminated => {
                return Err(anyhow!("Context state failed or terminated"));
            }
            _ => {}
        }
    }

    let result = op_logic(&mut context, &mut mainloop);
    context.disconnect();
    result
}

pub fn find_pulse_devices() -> Result<(Vec<(String, String)>, Vec<(String, String)>)> {
    run_pulse_op(|context, mainloop| {
        let sources = Rc::new(RefCell::new(Vec::new()));
        let sinks = Rc::new(RefCell::new(Vec::new()));
        let lists_completed = Rc::new(RefCell::new(0));

        {
            let op_source = context.introspect().get_source_info_list({
                let sources = Rc::clone(&sources);
                let lists_completed = Rc::clone(&lists_completed);
                move |res| {
                    if let ListResult::Item(item) = res {
                        if let (Some(name_cstr), Some(desc_cstr)) = (item.name.as_ref(), item.description.as_ref()) {
                            let name = String::from_utf8_lossy(name_cstr.as_bytes()).to_string();
                            let desc = String::from_utf8_lossy(desc_cstr.as_bytes()).to_string();
                            tracing::info!(source_name = %name, source_desc = %desc, "Found PulseAudio Source");
                            sources.borrow_mut().push((desc, name));
                        }
                    } else {
                        *lists_completed.borrow_mut() += 1;
                    }
                }
            });

            let op_sink = context.introspect().get_sink_info_list({
                let sinks = Rc::clone(&sinks);
                let lists_completed = Rc::clone(&lists_completed);
                move |res| {
                    if let ListResult::Item(item) = res {
                        if let (Some(name_cstr), Some(desc_cstr)) = (item.name.as_ref(), item.description.as_ref()) {
                            let name = String::from_utf8_lossy(name_cstr.as_bytes()).to_string();
                            let desc = String::from_utf8_lossy(desc_cstr.as_bytes()).to_string();
                            tracing::info!(sink_name = %name, sink_desc = %desc, "Found PulseAudio Sink");
                            sinks.borrow_mut().push((desc, name));
                        }
                    } else {
                        *lists_completed.borrow_mut() += 1;
                    }
                }
            });

            while *lists_completed.borrow() < 2 {
                if mainloop.iterate(false) == IterateResult::Quit(Retval(0)) {
                     return Err(anyhow!("Mainloop quit while getting devices"));
                }
            }
            drop(op_source);
            drop(op_sink);
        }

        let final_sources = sources.borrow().clone();
        let final_sinks = sinks.borrow().clone();
        Ok((final_sources, final_sinks))
    })
}

pub fn load_pulse_loopback(source: &str, sink: &str) -> Result<u32> {
    let args = format!(r#"source="{}" sink="{}""#, source, sink);
    run_pulse_op(|context, mainloop| {
        let index = Rc::new(RefCell::new(None));
        {
            let op = context.introspect().load_module("module-loopback", &args, {
                let index_clone = Rc::clone(&index);
                move |idx| {
                    *index_clone.borrow_mut() = Some(idx);
                }
            });

            while op.get_state() == OperationState::Running {
                if mainloop.iterate(false) == IterateResult::Quit(Retval(0)) {
                    return Err(anyhow!("Mainloop quit while loading module"));
                }
            }
        }
        // Explicitly scope the borrow to ensure the RefMut guard is dropped before the closure ends.
        let result = index.borrow_mut().take();
        result.context("Failed to get module index")
    })
}

pub fn unload_pulse_loopback(module_index: u32) -> Result<()> {
    run_pulse_op(|context, mainloop| {
        let op = context.introspect().unload_module(module_index, |_| {});
        while op.get_state() == OperationState::Running {
            if mainloop.iterate(false) == IterateResult::Quit(Retval(0)) {
                return Err(anyhow!("Mainloop quit while unloading module"));
            }
        }
        Ok(())
    })
}