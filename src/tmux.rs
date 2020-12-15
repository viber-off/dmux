use anyhow::Result;
use regex::Regex;
use std::process::{Command, Output};
use tmux_interface::pane::PANE_ALL;
use tmux_interface::session::SESSION_ALL;
use tmux_interface::window::WINDOW_ALL;
use tmux_interface::{
    AttachSession, NewSession, NewWindow, SendKeys, Sessions, SplitWindow, SwitchClient,
    TmuxInterface, Windows,
};

pub fn has_tmux() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .unwrap()
        .status
        .success()
}

pub struct Tmux {
    sessions: Vec<Session>,
}

pub fn in_tmux() -> bool {
    std::env::var("TMUX").is_ok()
}

pub fn generate_layout() -> Result<()> {
    if let Ok(values) = TmuxInterface::new().list_windows(
        Some(false),
        Some("#{window_active} #{window_layout}"),
        None,
    ) {
        if let Some(layout) = values.split('\n').find(|l| l.starts_with('1')) {
            println!(
                "{}",
                layout
                    .split_whitespace()
                    .last()
                    .ok_or_else(|| anyhow!("layout invalid"))?
            );
            Ok(())
        } else {
            Err(anyhow!("No active tmux window"))
        }
    } else {
        Err(anyhow!("Couldn't get layouts"))
    }
}

fn target(session_name: &str, window_name: &str, pane: i32) -> String {
    format!(
        "{}:{}.{}",
        clean_for_target(session_name),
        clean_for_target(window_name),
        pane
    )
}

// tmux's target conventions `sess:wind.pane` break when `wind` has a value like `coc.nvim`
// once again, tmux is kinda annoying
fn clean_for_target(string: &str) -> String {
    let re = Regex::new(r"[\.\s]").unwrap();
    re.replace_all(string, "-").into_owned()
}

#[derive(Debug)]
pub struct WorkSpace {
    pub session_name: String,
    pub window_name: String,
    pub dir: String,
    pub layout: Layout,
    pub commands: Commands,
}

#[allow(dead_code)]
pub fn default_layout_checksum() -> String {
    "34ed,230x56,0,0{132x56,0,0,3,97x56,133,0,222}".to_string()
}

// Make this a result type around Tmux
pub fn setup_workspace(workspace: WorkSpace) -> Result<Tmux> {
    let mut tmux = Tmux::new();
    let to_be_deleted: Option<String>;
    let session: &mut Session;

    if let Some(sess) = tmux.find_session(workspace.session_name.as_str()) {
        to_be_deleted = None;
        session = sess;
    } else {
        session = tmux
            .create_session(workspace.session_name.as_str())
            .ok_or_else(|| anyhow!("could not create session"))?;

        let deletion = session
            .windows
            .first()
            .ok_or_else(|| anyhow!("No first tmux window"))?
            .name
            .clone();
        to_be_deleted = Some(deletion);
    }
    session.setup_workspace(workspace)?.attach()?;
    if let Some(delete_name) = to_be_deleted {
        session.remove_window(delete_name.as_str())?;
    }
    Ok(tmux)
}

impl Tmux {
    pub fn new() -> Tmux {
        Tmux {
            sessions: Session::all_sessions(),
        }
    }

    pub fn send_keys(
        session_name: &str,
        window_name: &str,
        pane: i32,
        keys: Vec<&str>,
    ) -> Result<Output, tmux_interface::Error> {
        let target = target(session_name, window_name, pane);
        let split = SendKeys {
            target_pane: Some(target.as_str()),
            ..Default::default()
        };
        TmuxInterface::new().send_keys(Some(&split), &keys)
    }

    fn find_session(&mut self, name: &str) -> Option<&mut Session> {
        for sess in self.sessions.iter_mut() {
            if sess.name == name {
                return Some(sess);
            }
        }
        None
    }

    #[allow(dead_code)]
    fn has_session(&self, name: &str) -> bool {
        self.sessions.iter().any(|s| s.name == name)
    }

    fn create_session(&mut self, name: &str) -> Option<&mut Session> {
        let mut tmux = TmuxInterface::new();
        let new_session = NewSession {
            detached: Some(true),
            session_name: Some(name),
            ..Default::default()
        };
        if tmux.new_session(Some(&new_session)).is_ok() {
            self.sessions = Session::all_sessions();
            self.find_session(name)
        } else {
            None
        }
    }

    // #[allow(dead_code)]
    // pub fn find_or_create_session(&mut self, name: &str) -> Option<&mut Session> {
    //     if self.has_session(name) {
    //         self.find_session(name)
    //     } else {
    //         self.create_session(name)
    //     }
    // }
}

pub struct Session {
    windows: Vec<Window>,
    name: String,
}

// break this out into it's own module / file
impl Session {
    pub fn remove_window(&mut self, window_name: &str) -> Result<Output, tmux_interface::Error> {
        TmuxInterface::new().kill_window(Some(false), Some(self.target(window_name, 0).as_str()))
    }

    fn target(&self, window_name: &str, pane: i32) -> String {
        target(self.name.as_str(), window_name, pane)
    }

    pub fn setup_workspace(&mut self, workspace: WorkSpace) -> Result<&mut Window> {
        if self.has_window(workspace.window_name.as_str()) {
            return self
                .find_window(workspace.window_name.as_str())
                .ok_or_else(|| anyhow!("window destroyed during operation"));
        }
        let window = self
            .create_window(workspace.window_name.as_str(), workspace.dir.as_str())
            .ok_or_else(|| anyhow!("could not create window"))?;
        window.setup_layout(workspace.layout, workspace.dir.as_str())?;
        window.initial_command(workspace.commands)?;
        Ok(window)
    }

    pub fn all_sessions() -> Vec<Session> {
        let sessions = Sessions::get(SESSION_ALL).unwrap();
        Session::from_interface_list(sessions)
    }

    pub fn from_interface_list(sessions: tmux_interface::Sessions) -> Vec<Session> {
        sessions.into_iter().map(Session::from_interface).collect()
    }

    pub fn from_interface(session: tmux_interface::Session) -> Session {
        let name = clean_for_target(session.name.unwrap().as_str());
        Session {
            windows: Window::all_in_session(name.as_str()),
            name,
        }
    }

    fn find_window(&mut self, name: &str) -> Option<&mut Window> {
        for win in self.windows.iter_mut() {
            if win.name == name {
                return Some(win);
            }
        }
        None
    }

    fn has_window(&self, name: &str) -> bool {
        self.windows.iter().any(|w| w.name == name)
    }

    #[allow(dead_code)]
    pub fn find_or_create_window(&mut self, window_name: &str, dir: &str) -> Option<&mut Window> {
        if self.has_window(window_name) {
            self.find_window(window_name)
        } else {
            self.create_window(window_name, dir)
        }
    }

    fn create_window(&mut self, window_name: &str, dir: &str) -> Option<&mut Window> {
        let window_name = clean_for_target(window_name);
        let window = NewWindow {
            window_name: Some(window_name.as_str()),
            target_window: Some(self.name.as_str()),
            cwd: Some(dir),
            detached: Some(true),
            ..Default::default()
        };
        // Yuck, I really hate this but tmux interface returns a string from new_window
        if TmuxInterface::new().new_window(Some(&window)).is_ok() {
            self.windows = Window::all_in_session(self.name.as_str());
            self.find_window(window_name.as_str())
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct Layout {
    // I wouldn't need two things here if I could just parse the tmux layout checksum
    pub window_count: i32,
    pub layout_string: String,
}

pub type Commands = Vec<String>;

pub struct Window {
    panes: Vec<Pane>,
    session_name: String,
    number_of_panes: i32,
    name: String,
}

impl Window {
    fn from_interface(win: tmux_interface::Window, session_name: String) -> Window {
        let name = session_name.clone();
        let panes =
            tmux_interface::Panes::get(win.name.clone().unwrap().as_str(), PANE_ALL).unwrap();
        Window {
            panes: Pane::from_interface_list(
                panes,
                name.as_str(),
                win.name.clone().unwrap().as_str(),
            ),
            session_name,
            number_of_panes: win.panes.unwrap() as i32,
            name: win.name.unwrap(),
        }
    }

    pub fn all_in_session(session_name: &str) -> Vec<Window> {
        let windows = Windows::get(session_name, WINDOW_ALL).unwrap();
        windows
            .into_iter()
            .map(|w| Window::from_interface(w, String::from(session_name)))
            .collect()
    }

    pub fn send_keys(&self, keys: Vec<&str>) -> Result<Output, tmux_interface::Error> {
        Tmux::send_keys(self.session_name.as_str(), self.name.as_str(), 0, keys)
    }

    fn target(&self, pane: i32) -> String {
        target(self.session_name.as_str(), self.name.as_str(), pane)
    }

    fn split_window(&mut self, dir: &str) -> Result<String, tmux_interface::Error> {
        let target = self.target(0);
        let split = SplitWindow {
            cwd: Some(dir),
            target_pane: Some(target.as_str()),
            ..Default::default()
        };
        let mut tmux = TmuxInterface::new();
        let split_result = tmux.split_window(Some(&split));
        self.reload_panes();
        split_result
    }

    pub fn setup_layout(&mut self, layout: Layout, dir: &str) -> Result<Output> {
        // let lay = layout.layout_string.parse::<tmux_interface::Layout>();

        self.reload_panes();
        if self.number_of_panes < layout.window_count {
            for _x in self.number_of_panes..layout.window_count {
                self.split_window(dir)?;
            }
        }
        let tmux_command = format!(
            "tmux select-layout -t {} \"{}\"",
            self.target(0),
            layout.layout_string
        );
        self.reload_panes();
        Ok(self.send_keys(vec![tmux_command.as_str(), "Enter"])?)
    }

    fn get_pane(&mut self, pane: i32) -> Option<&Pane> {
        self.panes.iter().find(|p| p.index == pane)
    }

    pub fn attach(&self) -> Result<Output, tmux_interface::Error> {
        let target = self.target(0);
        if in_tmux() {
            let select = SwitchClient {
                target_session: Some(target.as_str()),
                ..Default::default()
            };
            let mut tmux = TmuxInterface::new();
            tmux.switch_client(Some(&select))
        } else {
            let attach = AttachSession {
                target_session: Some(&target),
                ..Default::default()
            };
            let mut tmux = TmuxInterface::new();
            tmux.attach_session(Some(&attach))
        }
    }

    // make this return a result
    pub fn initial_command(&mut self, commands: Commands) -> Result<()> {
        for (pane, command) in commands.iter().enumerate() {
            if let Some(pane) = self.get_pane(pane as i32) {
                pane.send_keys(vec![command.as_str(), "Enter"])?;
            }
        }
        Ok(())
    }

    fn reload_panes(&mut self) {
        let target = target(self.session_name.as_str(), self.name.as_str(), 0);
        let panes = tmux_interface::Panes::get(target.as_str(), PANE_ALL).unwrap();
        self.panes =
            Pane::from_interface_list(panes, self.session_name.as_str(), self.name.as_str());
    }
}

#[derive(Debug)]
struct Pane {
    session_name: String,
    window_name: String,
    index: i32,
}

impl Pane {
    pub fn send_keys(&self, keys: Vec<&str>) -> Result<Output, tmux_interface::Error> {
        Tmux::send_keys(
            self.session_name.as_str(),
            self.window_name.as_str(),
            self.index,
            keys,
        )
    }

    pub fn from_interface_list(
        panes: tmux_interface::Panes,
        session_name: &str,
        window_name: &str,
    ) -> Vec<Pane> {
        panes
            .into_iter()
            .map(|p| Pane::from_interface(p, session_name, window_name))
            .collect()
    }

    pub fn from_interface(
        interface: tmux_interface::Pane,
        session_name: &str,
        window_name: &str,
    ) -> Pane {
        Pane {
            index: interface.index.unwrap() as i32,
            session_name: session_name.to_string(),
            window_name: window_name.to_string(),
        }
    }
}
