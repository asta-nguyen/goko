import { call } from './call'

export const settingsService = {
  get: (key: string) => call<string | null>('settings_get', { key }),
  set: (key: string, value: string) => call<void>('settings_set', { key, value }),
}

export type SettingsService = typeof settingsService
