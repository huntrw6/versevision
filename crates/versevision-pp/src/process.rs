//! `propresenter/process.py`: is ProPresenter running on this computer?

use sysinfo::{ProcessesToUpdate, System};

/// The question the pipeline asks. A test answers it by hand.
pub trait ProcessProbe: Send + Sync {
    fn is_running(&self) -> bool;
}

/// The names the app has (`ProPresenter` on macOS, `ProPresenter.exe` on Windows), compared without case.
pub fn is_propresenter_name(name: &str) -> bool {
    let folded = versevision_core::references::pytext::casefold(name);
    folded == "propresenter" || folded == "propresenter.exe"
}

/// Looks at the processes of this computer.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemProbe;

impl ProcessProbe for SystemProbe {
    fn is_running(&self) -> bool {
        let Ok(mut system) = System::new() else {
            return false;
        };
        system.refresh_processes(ProcessesToUpdate::All, true);
        system
            .processes()
            .values()
            .any(|p| is_propresenter_name(&p.name().to_string_lossy()))
    }
}

/// A fixed answer (tests, hosts that know better).
#[derive(Debug)]
pub struct FixedProbe(pub std::sync::atomic::AtomicBool);

impl FixedProbe {
    pub fn new(running: bool) -> Self {
        FixedProbe(std::sync::atomic::AtomicBool::new(running))
    }
    pub fn set(&self, running: bool) {
        self.0.store(running, std::sync::atomic::Ordering::SeqCst);
    }
}

impl ProcessProbe for FixedProbe {
    fn is_running(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_match_without_case() {
        assert!(is_propresenter_name("ProPresenter"));
        assert!(is_propresenter_name("propresenter.EXE"));
        assert!(!is_propresenter_name("ProPresenter Helper"));
    }
}
