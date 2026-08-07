//! Turnstone's first settings projection provider.
//!
//! The provider adapts application-owned settings to Genet's host-facing
//! settings contract. It does not become a second store or a global registry.

use std::io;
use std::path::{Path, PathBuf};

use genet_host_api::settings::{
    SettingControl, SettingMovement, SettingMutability, SettingScope, SettingSecurity, SettingSpec,
    SettingValue, SettingsError, SettingsProvider,
};
use genet_host_api::tile::SettingsRef;
use session_runtime::{ApplicationSettings, load_application_settings, save_application_settings};

/// The application appearance page exposed by Turnstone's host shell.
pub const APPEARANCE_REFERENCE: &str = "pelt/appearance";

/// Adapts Turnstone's application settings store to the host projection.
pub struct ApplicationSettingsProvider {
    data_root: PathBuf,
    settings: ApplicationSettings,
}

impl ApplicationSettingsProvider {
    /// Load application settings from the data root, using typed defaults when
    /// the application has not written its settings file yet.
    pub fn load(data_root: impl Into<PathBuf>) -> io::Result<Self> {
        let data_root = data_root.into();
        let settings = load_application_settings(&data_root)?.unwrap_or_default();
        Ok(Self {
            data_root,
            settings,
        })
    }

    /// Construct a provider around already-loaded settings.
    pub fn from_settings(data_root: impl Into<PathBuf>, settings: ApplicationSettings) -> Self {
        Self {
            data_root: data_root.into(),
            settings,
        }
    }

    /// Inspect the typed application owner behind the projection.
    pub fn settings(&self) -> &ApplicationSettings {
        &self.settings
    }

    /// Return the data root used for persistence.
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    fn check_reference(reference: &SettingsRef) -> Result<(), SettingsError> {
        if reference.0 == APPEARANCE_REFERENCE {
            Ok(())
        } else {
            Err(SettingsError::UnsupportedReference(reference.clone()))
        }
    }

    fn application_text_spec(
        id: &str,
        label: &str,
        value: Option<&String>,
        movement: SettingMovement,
    ) -> SettingSpec {
        SettingSpec {
            id: id.into(),
            label: label.into(),
            scope: SettingScope::Application,
            movement,
            mutability: SettingMutability::Live,
            security: SettingSecurity::Ordinary,
            control: SettingControl::Text,
            value: SettingValue::Text(value.cloned().unwrap_or_default()),
        }
    }

    fn save(&self) -> Result<(), SettingsError> {
        save_application_settings(&self.data_root, &self.settings)
            .map_err(|error| SettingsError::Storage(error.to_string()))
    }
}

impl SettingsProvider for ApplicationSettingsProvider {
    fn describe(&self, reference: &SettingsRef) -> Result<Vec<SettingSpec>, SettingsError> {
        Self::check_reference(reference)?;
        Ok(vec![
            Self::application_text_spec(
                "theme.id",
                "Theme",
                self.settings.theme_id.as_ref(),
                SettingMovement::PersonaSynced,
            ),
            Self::application_text_spec(
                "theme.mode",
                "Theme mode",
                self.settings.theme_mode.as_ref(),
                SettingMovement::PersonaSynced,
            ),
            SettingSpec {
                id: "ui.zoom".into(),
                label: "UI zoom".into(),
                scope: SettingScope::Application,
                movement: SettingMovement::LocalOnly,
                mutability: SettingMutability::Live,
                security: SettingSecurity::Ordinary,
                control: SettingControl::Number {
                    min: Some(0.5),
                    max: Some(3.0),
                    step: Some(0.05),
                },
                value: SettingValue::Number(f64::from(self.settings.ui_zoom)),
            },
        ])
    }

    fn apply(
        &mut self,
        reference: &SettingsRef,
        setting_id: &str,
        value: SettingValue,
    ) -> Result<(), SettingsError> {
        Self::check_reference(reference)?;

        match (setting_id, value) {
            ("theme.id", SettingValue::Text(value)) => {
                self.settings.theme_id = (!value.is_empty()).then_some(value);
            }
            ("theme.mode", SettingValue::Text(value)) => {
                self.settings.theme_mode = (!value.is_empty()).then_some(value);
            }
            ("ui.zoom", SettingValue::Number(value))
                if value.is_finite() && (0.5..=3.0).contains(&value) =>
            {
                self.settings.ui_zoom = value as f32;
            }
            ("theme.id" | "theme.mode", other) => {
                return Err(SettingsError::InvalidValue {
                    setting_id: setting_id.into(),
                    message: format!("expected Text, got {other:?}"),
                });
            }
            ("ui.zoom", other) => {
                return Err(SettingsError::InvalidValue {
                    setting_id: setting_id.into(),
                    message: format!("expected Number in 0.5..=3.0, got {other:?}"),
                });
            }
            (other, _) => return Err(SettingsError::UnknownSetting(other.into())),
        }

        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "turnstone-settings-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn appearance_projection_describes_owner_axes_and_controls() {
        let provider = ApplicationSettingsProvider::from_settings(
            scratch_root("describe"),
            ApplicationSettings::default(),
        );
        let specs = provider
            .describe(&SettingsRef(APPEARANCE_REFERENCE.into()))
            .unwrap();

        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].movement, SettingMovement::PersonaSynced);
        assert_eq!(specs[0].control, SettingControl::Text);
        assert_eq!(specs[2].scope, SettingScope::Application);
        assert_eq!(
            specs[2].control,
            SettingControl::Number {
                min: Some(0.5),
                max: Some(3.0),
                step: Some(0.05),
            }
        );
    }

    #[test]
    fn typed_writes_update_the_owner_and_persist() {
        let root = scratch_root("apply");
        let reference = SettingsRef(APPEARANCE_REFERENCE.into());
        let mut provider = ApplicationSettingsProvider::load(&root).unwrap();

        provider
            .apply(
                &reference,
                "theme.id",
                SettingValue::Text("theme:night".into()),
            )
            .unwrap();
        provider
            .apply(&reference, "ui.zoom", SettingValue::Number(1.25))
            .unwrap();

        assert_eq!(provider.settings().theme_id.as_deref(), Some("theme:night"));
        assert_eq!(provider.settings().ui_zoom, 1.25);
        let loaded = load_application_settings(&root).unwrap().unwrap();
        assert_eq!(loaded.theme_id.as_deref(), Some("theme:night"));
        assert_eq!(loaded.ui_zoom, 1.25);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_reference_and_value_do_not_write() {
        let root = scratch_root("invalid");
        let mut provider = ApplicationSettingsProvider::load(&root).unwrap();

        assert!(matches!(
            provider.describe(&SettingsRef("wrong/page".into())),
            Err(SettingsError::UnsupportedReference(_))
        ));
        assert!(matches!(
            provider.apply(
                &SettingsRef(APPEARANCE_REFERENCE.into()),
                "ui.zoom",
                SettingValue::Number(4.0)
            ),
            Err(SettingsError::InvalidValue { .. })
        ));
        assert!(!session_runtime::application_settings_exist(&root));
        let _ = std::fs::remove_dir_all(root);
    }
}
