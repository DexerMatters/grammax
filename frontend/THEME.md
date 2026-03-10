# Tailwind Theme Configuration

This file documents the theme colors and semantic class names used throughout the Tree component and settings system.

## Theme System

The application includes a comprehensive theming system that supports:
- **Dark/Light mode switching** with auto-detection of system preference
- **Accent color themes** (default green, warm gold, cool cyan)
- **Font size adjustments** (small, medium, large)

All theme settings are persisted to localStorage and available via the Settings dialog (⚙️ button in the header).

### Using the Theme

Access the current theme configuration through the `useTheme` hook:

```tsx
import { useTheme } from './context/ThemeContext';

function MyComponent() {
  const { config, updateConfig, effectiveTheme } = useTheme();
  
  // config contains: theme, accentColor, fontSize
  // updateConfig to change settings
  // effectiveTheme is 'dark' or 'light' (resolved from 'auto')
}
```

## Color Palette

### Tokens (Gold/Tan - #d8a878 / dark, #b87f54 / light)
- `text-token` - Token text color
- `border-token-border` - Token border (with opacity)
- `shadow-token` - Token box shadow

### Errors
- **Unexpected** (Pink / dark: #ff8899, light: #dc3545)
  - `text-error-unexpected`
  - `border-error-unexpected-border`
  - `border-error-unexpected-border-hover`
  - `shadow-error`

- **Missing** (Gold / dark: #ffd700, light: #ffc107)
  - `text-error-missing`

- **Incomplete** (Cyan / dark: #66ddff, light: #00bcd4)
  - `text-error-incomplete`

### Branch/Rules (Green - default, changes with accent)
- `text-branch` - Rule name color (changes with accent color setting)
- `border-branch-border` - Branch line borders
- `border-branch-border-light` - Light branch lines
- `shadow-branch` - Branch box shadow
- `text-text-success` - Alternative branch color for quotes

#### Accent Color Variants
- **Default (Green)**: `#8bdb8b` (dark), `#4caf50` (light)
- **Warm (Gold)**: `#d8a878` (dark), `#b87f54` (light)
- **Cool (Cyan)**: `#66ddff` (dark), `#00bcd4` (light)

### Field Labels (Cyan - changes with accent)
- `text-field` - Field label text
- `border-field-border` - Field border
- `border-field-border-light` - Light field border
- `bg-field` - Field background
- `shadow-field` - Field box shadow

### Background Colors
- `bg-bg-base` - Main background (#1a1a1a dark, #f8f9fa light)
- `bg-bg-base-hover` - Hover state with transparency
- `bg-bg-darker` - Darker background (#2a2a2a dark, #eeeeee light)

### Text Colors
- `text-text-muted` - Muted text (#999 dark, #757575 light)
- `text-text-subtle` - Subtle text (#666 dark, #9e9e9e light)
- `text-text-success` - Success/green text (same as branch, changes with accent)

## Configuration File

The theme is defined in:
- `frontend/tailwind.config.ts` - Tailwind CSS configuration with CSS variable references
- `frontend/src/styles/theme.css` - CSS variables for dark/light modes and accent colors
- `frontend/src/context/ThemeContext.tsx` - React context managing theme state

### Adding New Colors

To add a new color to the theme:

1. **Add CSS variables** in `frontend/src/styles/theme.css`:
   ```css
   :root, :root.dark {
     --color-my-new-color: 100 150 200;
   }
   
   :root.light {
     --color-my-new-color: 50 100 150;
   }
   ```

2. **Reference in Tailwind config** in `frontend/tailwind.config.ts`:
   ```typescript
   'my-new': 'rgb(var(--color-my-new-color) / <alpha-value>)',
   ```

3. **Use in components**:
   ```tsx
   <div className="text-my-new">Text with new color</div>
   ```

## Theme Switching

Users can switch themes via the Settings dialog:

1. Click the ⚙️ **Settings** button in the top-right header
2. Choose theme: Auto (system), Dark, or Light
3. Choose accent color: Default (green), Warm (gold), or Cool (cyan)
4. Adjust font size if needed
5. Click **Apply** to save

Settings are automatically persisted to localStorage.

## CSS Variables Architecture

All colors are defined as CSS variables in RGB format (without alpha) for flexibility:

```css
--color-token: 216 168 120;  /* Can use with opacity: rgb(var(--color-token) / 0.5) */
```

This allows Tailwind to apply opacity values dynamically:
```tsx
className="border-2 border-token-border"  /* Uses default opacity */
className="border-2 border-token-border/50"  /* Override with 50% opacity */
```

## Light Mode Colors

When switching to light mode, colors are adjusted for readability and accessibility on light backgrounds:

- **Backgrounds**: Light grays (#f8f9fa to #eeeeee)
- **Text**: Dark grays (#757575 to #9e9e9e)
- **Accents**: Darker, more saturated versions for contrast
- **Errors**: Standard Material Design colors

## Browser Support

The theming system uses:
- CSS custom properties (CSS variables) - all modern browsers
- `prefers-color-scheme` media query - all modern browsers
- localStorage - all modern browsers
- Tailwind CSS - all modern browsers

For older browsers, consider adding fallbacks or a polyfill.

