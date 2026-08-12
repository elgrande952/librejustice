pub mod theme_settings;

#[cfg(feature = "hydrate")]
pub use theme_settings::sync_auth_theme;
pub use theme_settings::ThemeSettings;
