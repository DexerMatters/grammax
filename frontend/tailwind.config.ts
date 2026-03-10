import type { Config } from 'tailwindcss'

export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        // Token colors (gold/tan) - light mode
        token: {
          DEFAULT: 'rgb(var(--color-token))',
          border: 'rgb(var(--color-token-border) / 0.6)',
          light: 'rgb(var(--color-token-light) / 0.15)',
          shadow: 'rgb(var(--color-token-shadow) / 0.1)',
          text: 'rgb(var(--color-token))',
        },
        // Error states
        error: {
          unexpected: 'rgb(var(--color-error-unexpected))',
          'unexpected-border': 'rgb(var(--color-error-unexpected-border) / 0.6)',
          'unexpected-border-hover': 'rgb(var(--color-error-unexpected-border-hover))',
          'unexpected-shadow': 'rgb(var(--color-error-unexpected-shadow) / 0.05)',
          missing: 'rgb(var(--color-error-missing))',
          incomplete: 'rgb(var(--color-error-incomplete))',
        },
        // Branch/Rule lines (green)
        branch: {
          DEFAULT: 'rgb(var(--color-branch))',
          light: 'rgb(var(--color-branch-light) / 0.1)',
          border: 'rgb(var(--color-branch-border) / 0.6)',
          'border-light': 'rgb(var(--color-branch-border-light) / 0.4)',
          text: 'rgb(var(--color-branch))',
        },
        // Field colors (cyan)
        field: {
          DEFAULT: 'rgb(var(--color-field))',
          border: 'rgb(var(--color-field-border) / 0.5)',
          'border-hover': 'rgb(var(--color-field-border-hover) / 0.6)',
          'border-light': 'rgb(var(--color-field-border-light) / 0.4)',
          text: 'rgb(var(--color-field))',
          bg: 'rgb(var(--color-field-bg) / 0.4)',
        },
        // Background colors
        'bg': {
          base: 'rgb(var(--color-bg-base))',
          'base-hover': 'rgb(var(--color-bg-base-hover) / 0.5)',
          darker: 'rgb(var(--color-bg-darker))',
        },
        // Text colors
        'text': {
          muted: 'rgb(var(--color-text-muted))',
          subtle: 'rgb(var(--color-text-subtle))',
          success: 'rgb(var(--color-text-success))',
          'error-unexpected': 'rgb(var(--color-error-unexpected))',
          'error-missing': 'rgb(var(--color-error-missing))',
          'error-incomplete': 'rgb(var(--color-error-incomplete))',
        },
      },
      boxShadow: {
        token: '0 0 8px rgb(var(--color-token-shadow))',
        error: '0 0 8px rgb(var(--color-error-unexpected-shadow))',
        'error-hover': '0 0 16px rgb(var(--color-error-unexpected-shadow) / 0.3)',
        branch: '0 0 8px rgb(var(--color-branch-light))',
        field: '0 0 8px rgb(var(--color-field-border) / 0.05)',
      },
    },
  },
  plugins: [],
} satisfies Config

