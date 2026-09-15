use std::collections::{HashMap, HashSet};

use sysinfo::{Pid, ProcessesToUpdate, System};

pub struct ProcTree {
    sys: System,
}

impl ProcTree {
    pub fn new() -> ProcTree {
        ProcTree { sys: System::new() }
    }

    pub fn refresh(&mut self) {
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            sysinfo::ProcessRefreshKind::nothing().with_cwd(sysinfo::UpdateKind::Always),
        );
    }

    pub fn descendants_cwds(&mut self, root_pid: u32) -> Vec<(u32, String)> {
        self.refresh();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, proc_) in self.sys.processes() {
            if let Some(parent) = proc_.parent() {
                children
                    .entry(parent.as_u32())
                    .or_default()
                    .push(pid.as_u32());
            }
        }
        let mut out = Vec::new();
        let mut seen: HashSet<u32> = HashSet::new();
        let mut stack = vec![root_pid];
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            if let Some(p) = self.sys.process(Pid::from_u32(pid)) {
                if let Some(cwd) = p.cwd() {
                    let c = cwd.to_string_lossy().into_owned();
                    if !c.is_empty() {
                        out.push((pid, c));
                    }
                }
            }
            if let Some(kids) = children.get(&pid) {
                stack.extend(kids);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_own_process_with_cwd() {
        let mut tree = ProcTree::new();
        let me = std::process::id();
        let procs = tree.descendants_cwds(me);
        assert!(procs.iter().any(|(pid, _)| *pid == me));
        let cwd = std::env::current_dir().unwrap();
        let expect = cwd.to_string_lossy();
        assert!(
            procs.iter().any(|(_, c)| *c == expect),
            "own cwd must be visible"
        );
    }

    #[test]
    fn unknown_root_yields_nothing() {
        let mut tree = ProcTree::new();
        assert!(tree.descendants_cwds(u32::MAX).is_empty());
    }
}
