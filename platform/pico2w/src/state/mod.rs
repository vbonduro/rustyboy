mod in_game_menu;
mod loading;
mod main_menu;
mod rom_list;
mod running;
mod settings;
mod wifi_menu;

const MENU_POLL_MS: u64 = 4;
const PORTAL_POLL_MS: u64 = 16;

pub use in_game_menu::InGameMenuState;
pub use loading::LoadingState;
pub use main_menu::MainMenuState;
pub use rom_list::RomListState;
pub use running::RunningState;
pub use settings::SettingsState;
pub use wifi_menu::WifiMenuState;
