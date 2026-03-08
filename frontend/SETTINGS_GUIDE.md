# Settings & Theme System Setup Guide

## Overview

The Grammax frontend now includes a complete theming system with user-configurable settings. This guide explains the architecture and how to add new configurable options.

## File Structure

```
frontend/src/
├── context/
│   └── ThemeContext.tsx          # Theme state management & persistence
├── components/
│   └── SettingsDialog.tsx        # Settings UI dialog
├── styles/
│   └── theme.css                 # CSS variables for colors
├── App.tsx                        # Main app with settings button
├── index.css                      # Imports theme.css
└── main.tsx                       # Wraps app with ThemeProvider
```

## How the Theming System Works

### 1. ThemeContext (State Management)
- Located in `frontend/src/context/ThemeContext.tsx`
- Manages theme configuration state
- Persists settings to localStorage
- Applies theme classes to the document root
- Provides `useTheme()` hook for components to access theme

**Configuration object:**
```typescript
interface ThemeConfig {
  theme: 'dark' | 'light' | 'auto';      // Display theme
  accentColor: 'default' | 'warm' | 'cool'; // Primary accent
  fontSize: 'small' | 'medium' | 'large';  // Global font size
}
```

### 2. CSS Variables (Dynamic Colors)
- Located in `frontend/src/styles/theme.css`
- Defines RGB color variables for each mode
- Dark mode is the default
- Light mode variables override in `.light` selector
- Uses `data-accent-color` and `data-font-size` attributes for variations

**Example:**
```css
:root, :root.dark {
  --color-branch: 139 219 139;  /* Dark green */
}

:root.light {
  --color-branch: 76 175 80;    /* Light green */
}

:root[data-accent-color="warm"] {
  --color-branch: 216 168 120;  /* Warm accent */
}
```

### 3. Tailwind Integration
- Located in `frontend/tailwind.config.ts`
- References CSS variables instead of hardcoded colors
- Uses `rgb(var(--color-name) / <alpha-value>)` syntax
- Allows dynamic color application with opacity

**Example:**
```typescript
colors: {
  branch: {
    DEFAULT: 'rgb(var(--color-branch) / <alpha-value>)',
    // ... other variants
  }
}
```

### 4. Settings Dialog
- Located in `frontend/src/components/SettingsDialog.tsx`
- Modal UI for changing settings
- Uses Framer Motion for smooth animations
- Accessible radio buttons for each option
- Apply/Cancel buttons to manage changes

### 5. App Integration
- Settings button in header (`frontend/src/App.tsx`)
- ThemeProvider wraps entire app (`frontend/src/main.tsx`)
- SettingsDialog component integrated into App

## Adding New Settings

### Step 1: Update ThemeConfig Interface
In `frontend/src/context/ThemeContext.tsx`:

```typescript
export interface ThemeConfig {
  theme: Theme;
  accentColor: 'default' | 'warm' | 'cool';
  fontSize: 'small' | 'medium' | 'large';
  // ADD NEW OPTION HERE:
  myNewOption: 'opt1' | 'opt2' | 'opt3';
}
```

### Step 2: Update CSS Variables
In `frontend/src/styles/theme.css`:

```css
:root {
  /* ... existing vars ... */
  --my-new-option-opt1: value;
  --my-new-option-opt2: value;
  --my-new-option-opt3: value;
}

:root[data-my-new-option="opt1"] {
  /* Apply changes for opt1 */
}
```

### Step 3: Add UI to Settings Dialog
In `frontend/src/components/SettingsDialog.tsx`:

```tsx
<div>
  <label className="block text-sm font-medium text-text-muted mb-3">
    My New Option
  </label>
  <div className="space-y-2">
    {(['opt1', 'opt2', 'opt3'] as const).map((option) => (
      <label key={option} className="flex items-center gap-3 p-2 rounded hover:bg-bg-base-hover cursor-pointer transition-colors">
        <input
          type="radio"
          name="myNewOption"
          value={option}
          checked={localConfig.myNewOption === option}
          onChange={(e) => setLocalConfig({ ...localConfig, myNewOption: e.target.value })}
          className="w-4 h-4 accent-branch"
        />
        <span className="text-sm text-text-muted">{option}</span>
      </label>
    ))}
  </div>
</div>
```

### Step 4: Apply Theme in Component
Use the setting value to adjust styling:

```tsx
const { config } = useTheme();

// Use config.myNewOption to conditionally apply styles
className={config.myNewOption === 'opt1' ? 'some-class' : 'other-class'}
```

## Default Settings

- **Theme**: `auto` (follows system preference)
- **Accent Color**: `default` (green)
- **Font Size**: `medium` (14px)

## Storage

All settings are stored in `localStorage` under the key `grammax-theme-config` as a JSON string. Users can manually clear localStorage to reset to defaults.

## Testing Theme Changes

### Programmatically (in components):
```tsx
import { useTheme } from './context/ThemeContext';

function TestComponent() {
  const { config, updateConfig } = useTheme();
  
  return (
    <button onClick={() => updateConfig({ theme: 'light' })}>
      Switch to Light Mode
    </button>
  );
}
```

### Manually:
1. Click ⚙️ Settings button in header
2. Select options
3. Click Apply
4. Settings persist across page reloads

## Browser Compatibility

- Requires modern browser with CSS custom properties support
- CSS Grid/Flexbox for layout
- ES6+ JavaScript features
- localStorage API for persistence

## Performance Considerations

- CSS variables are applied at the root level for efficiency
- Theme changes apply globally via class/attribute selectors
- No per-component theme lookups needed (after initial context setup)
- Minimal re-renders due to ThemeContext memoization
