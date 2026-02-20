use std::collections::HashSet;

use crate::discovery::{AzureContainerApp, DockerContainer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceMenuScreen {
    MainMenu,
    DockerDiscovery,
    AzureDiscovery,
    FileInput,
    CommandInput,
}

pub const MAIN_MENU_ITEMS: &[&str] = &[
    "Docker Container",
    "File (tail)",
    "Azure Container App",
    "Custom Command",
];

pub struct SourceMenuState {
    pub screen: SourceMenuScreen,
    pub main_cursor: usize,
    pub discovery_cursor: usize,
    pub selected: HashSet<usize>,
    pub text_input: String,
    pub docker_containers: Vec<DockerContainer>,
    pub azure_apps: Vec<AzureContainerApp>,
    pub docker_loading: bool,
    pub azure_loading: bool,
    pub docker_error: Option<String>,
    pub azure_error: Option<String>,
}

impl SourceMenuState {
    pub fn new() -> Self {
        SourceMenuState {
            screen: SourceMenuScreen::MainMenu,
            main_cursor: 0,
            discovery_cursor: 0,
            selected: HashSet::new(),
            text_input: String::new(),
            docker_containers: Vec::new(),
            azure_apps: Vec::new(),
            docker_loading: false,
            azure_loading: false,
            docker_error: None,
            azure_error: None,
        }
    }

    pub fn reset(&mut self) {
        self.screen = SourceMenuScreen::MainMenu;
        self.main_cursor = 0;
        self.discovery_cursor = 0;
        self.selected.clear();
        self.text_input.clear();
        self.docker_containers.clear();
        self.azure_apps.clear();
        self.docker_loading = false;
        self.azure_loading = false;
        self.docker_error = None;
        self.azure_error = None;
    }

    pub fn discovery_item_count(&self) -> usize {
        match self.screen {
            SourceMenuScreen::DockerDiscovery => self.docker_containers.len(),
            SourceMenuScreen::AzureDiscovery => self.azure_apps.len(),
            _ => 0,
        }
    }

    pub fn toggle_selection(&mut self) {
        let count = self.discovery_item_count();
        if count > 0 && self.discovery_cursor < count {
            if self.selected.contains(&self.discovery_cursor) {
                self.selected.remove(&self.discovery_cursor);
            } else {
                self.selected.insert(self.discovery_cursor);
            }
        }
    }
}
