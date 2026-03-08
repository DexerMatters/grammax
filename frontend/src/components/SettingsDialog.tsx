import React, { useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { useTheme, type Theme, type ThemeConfig } from '../context/ThemeContext';

interface SettingsDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

export const SettingsDialog: React.FC<SettingsDialogProps> = ({ isOpen, onClose }) => {
  const { config, updateConfig } = useTheme();
  const [localConfig, setLocalConfig] = useState<ThemeConfig>(config);

  // Sync local config with global config when dialog opens
  React.useEffect(() => {
    if (isOpen) {
      setLocalConfig(config);
    }
  }, [isOpen, config]);

  const handleApply = () => {
    updateConfig(localConfig);
    onClose();
  };

  const handleCancel = () => {
    setLocalConfig(config);
    onClose();
  };

  const handleThemeChange = (theme: Theme) => {
    setLocalConfig({ ...localConfig, theme });
  };

  const handleAccentColorChange = (color: 'default' | 'warm' | 'cool') => {
    setLocalConfig({ ...localConfig, accentColor: color });
  };

  const handleFontSizeChange = (size: 'small' | 'medium' | 'large' | 'xlarge') => {
    setLocalConfig({ ...localConfig, fontSize: size });
  };

  return (
    <AnimatePresence>
      {isOpen && (
        <>
          {/* Backdrop */}
          <motion.div
            className="fixed inset-0 bg-black/50 z-40"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            onClick={handleCancel}
          />

          {/* Dialog */}
          <motion.div
            className="fixed left-1/2 top-1/2 z-50 mx-4 flex max-h-[90vh] w-full max-w-sm -translate-x-1/2 -translate-y-1/2 flex-col rounded-2xl border border-zinc-300/70 bg-bg-base shadow-2xl dark:border-white/10"
            initial={{ opacity: 0, scale: 0.95, y: -20 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.95, y: -20 }}
            transition={{ duration: 0.2 }}
          >
            {/* Header */}
            <div className="shrink-0 border-b border-zinc-300/70 px-6 py-4 dark:border-white/10">
              <h2 className="text-lg font-semibold text-branch">Settings</h2>
            </div>

            {/* Content */}
            <div className="px-6 py-4 space-y-6 overflow-y-auto flex-1">
              {/* Theme Selection */}
              <div>
                <label className="block text-sm font-medium text-text-muted mb-3">
                  Theme
                </label>
                <div className="space-y-2">
                  {(['auto', 'dark', 'light'] as const).map((theme) => (
                    <label
                      key={theme}
                      className="flex items-center gap-3 p-2 rounded hover:bg-bg-base-hover cursor-pointer transition-colors"
                    >
                      <input
                        type="radio"
                        name="theme"
                        value={theme}
                        checked={localConfig.theme === theme}
                        onChange={(e) => handleThemeChange(e.target.value as Theme)}
                        className="w-4 h-4"
                        style={{ accentColor: 'rgb(var(--color-branch))' }}
                      />
                      <span className="text-sm capitalize text-text-muted">
                        {theme === 'auto' ? 'Auto (System)' : theme}
                      </span>
                    </label>
                  ))}
                </div>
              </div>

              {/* Accent Color Selection */}
              <div>
                <label className="block text-sm font-medium text-text-muted mb-3">
                  Accent Color
                </label>
                <div className="space-y-2">
                  {(['default', 'warm', 'cool'] as const).map((color) => {
                    const colorPreview =
                      color === 'default' ? '#8bdb8b' :
                        color === 'warm' ? '#d8a878' :
                          '#66ddff';
                    return (
                      <label
                        key={color}
                        className="flex items-center gap-3 p-2 rounded hover:bg-bg-base-hover cursor-pointer transition-colors"
                      >
                        <input
                          type="radio"
                          name="accent"
                          value={color}
                          checked={localConfig.accentColor === color}
                          onChange={(e) =>
                            handleAccentColorChange(e.target.value as 'default' | 'warm' | 'cool')
                          }
                          className="w-4 h-4"
                          style={{ accentColor: 'rgb(var(--color-branch))' }}
                        />
                        <div className="flex items-center gap-2">
                          <div
                            className="w-4 h-4 rounded-full"
                            style={{ backgroundColor: colorPreview }}
                          />
                          <span className="text-sm capitalize text-text-muted">{color}</span>
                        </div>
                      </label>
                    );
                  })}
                </div>
              </div>

              {/* Font Size Selection */}
              <div>
                <label className="block text-sm font-medium text-text-muted mb-3">
                  Font Size
                </label>
                <div className="space-y-2">
                  {(['small', 'medium', 'large', 'xlarge'] as const).map((size) => {
                    const sizeLabel =
                      size === 'small' ? 'Small (12px)' :
                        size === 'medium' ? 'Medium (14px)' :
                          size === 'large' ? 'Large (16px)' :
                            'Extra Large (20px)'
                    return (
                      <label
                        key={size}
                        className="flex items-center gap-3 p-2 rounded hover:bg-bg-base-hover cursor-pointer transition-colors"
                      >
                        <input
                          type="radio"
                          name="fontSize"
                          value={size}
                          checked={localConfig.fontSize === size}
                          onChange={(e) =>
                            handleFontSizeChange(e.target.value as 'small' | 'medium' | 'large' | 'xlarge')
                          }
                          className="w-4 h-4"
                          style={{ accentColor: 'rgb(var(--color-branch))' }}
                        />
                        <span className="text-sm text-text-muted">{sizeLabel}</span>
                      </label>
                    );
                  })}
                </div>
              </div>

              {/* Display Options */}
              <div>
                <label className="block text-sm font-medium text-text-muted mb-3">
                  Display Options
                </label>
                <label className="flex items-center gap-3 p-2 rounded hover:bg-bg-base-hover cursor-pointer transition-colors">
                  <input
                    type="checkbox"
                    checked={localConfig.foldRelayNodes}
                    onChange={(e) =>
                      setLocalConfig({ ...localConfig, foldRelayNodes: e.target.checked })
                    }
                    className="w-4 h-4"
                    style={{ accentColor: 'rgb(var(--color-branch))' }}
                  />
                  <span className="text-sm text-text-muted">Fold relay nodes</span>
                </label>
              </div>
            </div>

            {/* Footer */}
            <div className="flex shrink-0 justify-end gap-3 border-t border-zinc-300/70 px-6 py-4 dark:border-white/10">
              <button
                onClick={handleCancel}
                className="rounded-lg border border-zinc-300/70 px-4 py-2 text-text-muted transition-colors hover:bg-bg-base-hover dark:border-white/10"
              >
                Cancel
              </button>
              <button
                onClick={handleApply}
                className="rounded-lg bg-branch px-4 py-2 font-medium text-white dark:text-black transition-opacity hover:opacity-90"
              >
                Apply
              </button>
            </div>
          </motion.div>
        </>
      )}
    </AnimatePresence>
  );
};
