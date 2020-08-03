use std::default::Default;
use std::f64;
use std::fs::{File, OpenOptions};
use std::io::prelude::*;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::Duration;

use chrono::Local;
use cursive::direction::Absolute;
use cursive::Vec2;
use notify::{watcher, RecursiveMode, Watcher};

use crate::command::{Command, CommandLineError};
use crate::habit::{Bit, Count, HabitWrapper, TrackEvent, ViewMode};
use crate::utils::{self, GRID_WIDTH, VIEW_HEIGHT, VIEW_WIDTH};

use crate::app::{App, MessageKind, StatusLine};

impl App {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let mut watcher = watcher(tx, Duration::from_secs(1)).unwrap();
        watcher.watch(utils::auto_habit_file(), RecursiveMode::Recursive);
        return App {
            habits: vec![],
            focus: 0,
            _file_watcher: watcher,
            file_event_recv: rx,
            view_month_offset: 0,
            message: "Type :add <habit-name> <goal> to get started, Ctrl-L to dismiss".into(),
        };
    }

    pub fn add_habit(&mut self, h: Box<dyn HabitWrapper>) {
        self.habits.push(h);
    }

    pub fn list_habits(&self) -> Vec<String> {
        self.habits.iter().map(|x| x.name()).collect::<Vec<_>>()
    }

    pub fn delete_by_name(&mut self, name: &str) {
        let old_len = self.habits.len();
        self.habits.retain(|h| h.name() != name);
        if old_len == self.habits.len() {
            self.message
                .set_message(format!("Could not delete habit `{}`", name))
        }
    }

    pub fn get_mode(&self) -> ViewMode {
        if self.habits.is_empty() {
            return ViewMode::Day;
        }
        return self.habits[self.focus].view_mode();
    }

    pub fn set_mode(&mut self, mode: ViewMode) {
        if !self.habits.is_empty() {
            self.habits[self.focus].set_view_mode(mode);
        }
    }

    pub fn set_view_month_offset(&mut self, offset: u32) {
        self.view_month_offset = offset;
        for v in self.habits.iter_mut() {
            v.set_view_month_offset(offset);
        }
    }

    pub fn sift_backward(&mut self) {
        self.view_month_offset += 1;
        for v in self.habits.iter_mut() {
            v.set_view_month_offset(self.view_month_offset);
        }
    }

    pub fn sift_forward(&mut self) {
        if self.view_month_offset > 0 {
            self.view_month_offset -= 1;
            for v in self.habits.iter_mut() {
                v.set_view_month_offset(self.view_month_offset);
            }
        }
    }

    pub fn set_focus(&mut self, d: Absolute) {
        match d {
            Absolute::Right => {
                if self.focus != self.habits.len() - 1 {
                    self.focus += 1;
                }
            }
            Absolute::Left => {
                if self.focus != 0 {
                    self.focus -= 1;
                }
            }
            Absolute::Down => {
                if self.focus + GRID_WIDTH < self.habits.len() - 1 {
                    self.focus += GRID_WIDTH;
                } else {
                    self.focus = self.habits.len() - 1;
                }
            }
            Absolute::Up => {
                if self.focus as isize - GRID_WIDTH as isize >= 0 {
                    self.focus -= GRID_WIDTH;
                } else {
                    self.focus = 0;
                }
            }
            Absolute::None => {}
        }
    }

    pub fn clear_message(&mut self) {
        self.message.clear();
    }

    pub fn status(&self) -> StatusLine {
        let today = chrono::Local::now().naive_local().date();
        let remaining = self.habits.iter().map(|h| h.remaining(today)).sum::<u32>();
        let total = self.habits.iter().map(|h| h.goal()).sum::<u32>();
        let completed = total - remaining;

        let timestamp = if self.view_month_offset == 0 {
            format!("{}", Local::now().naive_local().date().format("%d/%b/%y"),)
        } else {
            let months = self.view_month_offset;
            format!("{}", format!("{} months ago", months),)
        };

        StatusLine {
            0: format!(
                "Today: {} completed, {} remaining --{}--",
                completed,
                remaining,
                self.get_mode()
            ),
            1: timestamp,
        }
    }

    pub fn max_size(&self) -> Vec2 {
        let width = GRID_WIDTH * VIEW_WIDTH;
        let height = {
            if !self.habits.is_empty() {
                (VIEW_HEIGHT as f64 * (self.habits.len() as f64 / GRID_WIDTH as f64).ceil())
                    as usize
            } else {
                0
            }
        };
        Vec2::new(width, height + 2)
    }

    pub fn load_state() -> Self {
        let (regular_f, auto_f) = (utils::habit_file(), utils::auto_habit_file());
        let read_from_file = |file: PathBuf| -> Vec<Box<dyn HabitWrapper>> {
            if let Ok(ref mut f) = File::open(file) {
                let mut j = String::new();
                f.read_to_string(&mut j);
                return serde_json::from_str(&j).unwrap();
            } else {
                return Vec::new();
            }
        };

        let mut regular = read_from_file(regular_f);
        let auto = read_from_file(auto_f);
        regular.extend(auto);
        return App {
            habits: regular,
            ..Default::default()
        };
    }

    // this function does IO
    // TODO: convert this into non-blocking async function
    pub fn save_state(&self) {
        let (regular, auto): (Vec<_>, Vec<_>) = self.habits.iter().partition(|&x| !x.is_auto());
        let (regular_f, auto_f) = (utils::habit_file(), utils::auto_habit_file());

        let write_to_file = |data: Vec<&Box<dyn HabitWrapper>>, file: PathBuf| {
            let j = serde_json::to_string_pretty(&data).unwrap();
            match OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(file)
            {
                Ok(ref mut f) => f.write_all(j.as_bytes()).unwrap(),
                Err(_) => panic!("Unable to write!"),
            };
        };

        write_to_file(regular, regular_f);
        write_to_file(auto, auto_f);
    }

    pub fn parse_command(&mut self, result: Result<Command, CommandLineError>) {
        let mut _track = |name: &str, event: TrackEvent| {
            let target_habit = self
                .habits
                .iter_mut()
                .find(|x| x.name() == name && x.is_auto());
            if let Some(h) = target_habit {
                h.modify(Local::now().naive_local().date(), event);
            }
        };
        match result {
            Ok(c) => match c {
                Command::Add(name, goal, auto) => {
                    if let Some(_) = self.habits.iter().find(|x| x.name() == name) {
                        self.message.set_kind(MessageKind::Error);
                        self.message
                            .set_message(format!("Habit `{}` already exist", &name));
                        return;
                    }
                    let kind = if goal == Some(1) { "bit" } else { "count" };
                    if kind == "count" {
                        self.add_habit(Box::new(Count::new(name, goal.unwrap_or(0), auto)));
                    } else if kind == "bit" {
                        self.add_habit(Box::new(Bit::new(name, auto)));
                    }
                }
                Command::Delete(name) => {
                    self.delete_by_name(&name);
                    self.focus = 0;
                }
                Command::TrackUp(name) => {
                    _track(&name, TrackEvent::Increment);
                }
                Command::TrackDown(name) => {
                    _track(&name, TrackEvent::Decrement);
                }
                Command::Help(input) => {
                    if let Some(topic) = input.as_ref().map(String::as_ref) {
                        self.message.set_message(
                            match topic {
                                "a"     | "add" => "add <habit-name> [goal]     (alias: a)",
                                "aa"    | "add-auto" => "add-auto <habit-name> [goal]     (alias: aa)",
                                "d"     | "delete" => "delete <habit-name>     (alias: d)",
                                "mprev" | "month-prev" => "month-prev     (alias: mprev)",
                                "mnext" | "month-next" => "month-next     (alias: mnext)",
                                "tup"   | "track-up" => "track-up <auto-habit-name>     (alias: tup)",
                                "tdown" | "track-down" => "track-down <auto-habit-name>     (alias: tdown)",
                                "q"     | "quit" => "quit dijo",
                                "w"     | "write" => "write current state to disk   (alias: w)",
                                "h"|"?" | "help" => "help [<command>|commands|keys]     (aliases: h, ?)",
                                "cmds"  | "commands" => "add, add-auto, delete, month-{prev,next}, track-{up,down}, help, quit",
                                "keys" => "TODO", // TODO (view?)
                                _ => "unknown command or help topic.",
                            }
                        )
                    } else {
                        // TODO (view?)
                        self.message.set_message("help <command>|commands|keys")
                    }
                }
                Command::Quit | Command::Write => self.save_state(),
                Command::MonthNext => self.sift_forward(),
                Command::MonthPrev => self.sift_backward(),
                Command::Blank => {}
            },
            Err(e) => {
                self.message.set_message(e.to_string());
                self.message.set_kind(MessageKind::Error);
            }
        }
    }
}
