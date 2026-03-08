import React, { createContext, useContext, useEffect, useState } from 'react';

export type Theme = 'dark' | 'light' | 'auto';

export interface ThemeConfig {
  theme: Theme;
  accentColor: 'default' | 'warm' | 'cool';
  fontSize: 'small' | 'medium' | 'large' | 'xlarge';
  foldRelayNodes: boolean;
}

interface ThemeContextType {
  config: ThemeConfig;
  updateConfig: (partial: Partial<ThemeConfig>) => void;
  effectiveTheme: 'dark' | 'light';
}

const ThemeContext = createContext<ThemeContextType | undefined>(undefined);

const DEFAULT_CONFIG: ThemeConfig = {
  theme: 'auto',
  accentColor: 'default',
  fontSize: 'large',
  foldRelayNodes: true,
};

const STORAGE_KEY = 'grammax-theme-config';

export const ThemeProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [config, setConfig] = useState<ThemeConfig>(DEFAULT_CONFIG);
  const [effectiveTheme, setEffectiveTheme] = useState<'dark' | 'light'>('dark');

  // Load config from localStorage on mount
  useEffect(() => {
    try {
      const stored = localStorage.getItem(STORAGE_KEY);
      if (stored) {
        setConfig(JSON.parse(stored));
      }
    } catch (error) {
      console.error('Failed to load theme config:', error);
    }
  }, []);

  // Determine effective theme and apply it
  useEffect(() => {
    let theme: 'dark' | 'light' = 'dark';

    if (config.theme === 'auto') {
      theme = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
    } else {
      theme = config.theme;
    }

    setEffectiveTheme(theme);

    // Apply theme classes to document root
    const htmlElement = document.documentElement;
    if (theme === 'dark') {
      htmlElement.classList.add('dark');
      htmlElement.classList.remove('light');
    } else {
      htmlElement.classList.add('light');
      htmlElement.classList.remove('dark');
    }

    // Apply accent color
    htmlElement.setAttribute('data-accent-color', config.accentColor);

    // Apply font size
    htmlElement.setAttribute('data-font-size', config.fontSize);
  }, [config]);

  const updateConfig = (partial: Partial<ThemeConfig>) => {
    const newConfig = { ...config, ...partial };
    setConfig(newConfig);

    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(newConfig));
    } catch (error) {
      console.error('Failed to save theme config:', error);
    }
  };

  return (
    <ThemeContext.Provider value={{ config, updateConfig, effectiveTheme }}>
      {children}
    </ThemeContext.Provider>
  );
};

export const useTheme = () => {
  const context = useContext(ThemeContext);
  if (!context) {
    throw new Error('useTheme must be used within a ThemeProvider');
  }
  return context;
};
