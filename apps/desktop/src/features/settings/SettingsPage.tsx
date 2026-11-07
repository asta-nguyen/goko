import { KeyboardIcon, SunMoonIcon } from 'lucide-react'
import { useState } from 'react'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { cn } from '@/lib/utils'
import { resolveTheme, THEME_OPTIONS, THEME_SWATCH, type Theme, useTheme } from '../../components/use-theme'
import { useEditorVim } from '../editor/queries'
import { AcpAgentsSection } from './AcpAgentsSection'

const SECTIONS = [
  { id: 'editor', label: 'Editor' },
  { id: 'appearance', label: 'Appearance' },
  { id: 'acp-agents', label: 'ACP agents' },
] as const
type SettingsSectionId = (typeof SECTIONS)[number]['id']

function SettingRow({
  icon,
  label,
  description,
  control,
}: {
  icon: React.ReactNode
  label: React.ReactNode
  description: string
  control: React.ReactNode
}) {
  return (
    <div className="flex items-center justify-between gap-4">
      <div className="flex min-w-0 items-start gap-3">
        <span className="mt-0.5 shrink-0 text-muted" aria-hidden>
          {icon}
        </span>
        <div className="min-w-0">
          {label}
          <p className="m-0 mt-0.5 text-xs text-muted">{description}</p>
        </div>
      </div>
      {control}
    </div>
  )
}

export function SettingsPage() {
  const [section, setSection] = useState<SettingsSectionId>('editor')
  const [vim, setVim] = useEditorVim()
  const [theme, setTheme] = useTheme()

  return (
    <section aria-label="Settings" className="flex min-h-0 min-w-0 flex-1 overflow-hidden">
      <nav aria-label="Settings sections" className="w-44 shrink-0 overflow-y-auto border-r border-border bg-panel p-3">
        <ul className="m-0 flex list-none flex-col gap-0.5 p-0">
          {SECTIONS.map((item) => (
            <li key={item.id}>
              <button
                type="button"
                aria-current={section === item.id ? 'true' : undefined}
                onClick={() => setSection(item.id)}
                className={cn(
                  'w-full rounded-md px-2.5 py-1.5 text-left text-sm outline-none hover:bg-bg/60 focus-visible:ring-2 focus-visible:ring-ring',
                  section === item.id ? 'bg-secondary/70 font-medium text-text' : 'text-muted',
                )}
              >
                {item.label}
              </button>
            </li>
          ))}
        </ul>
      </nav>
      <div className="min-h-0 min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex max-w-2xl flex-col gap-4 p-6">
          <div>
            <h1 className="m-0 text-lg font-semibold tracking-tight">Settings</h1>
            <p className="m-0 mt-0.5 text-sm text-muted">
              {SECTIONS.find((item) => item.id === section)?.label}
            </p>
          </div>
          {section === 'editor' && (
            <Card>
              <CardHeader>
                <CardTitle>Editor</CardTitle>
                <CardDescription>Editing files in the workspace editor.</CardDescription>
              </CardHeader>
              <CardContent>
                <SettingRow
                  icon={<KeyboardIcon className="size-4" />}
                  label={
                    <Label htmlFor="settings-vim" className="text-sm font-medium">
                      Vim mode
                    </Label>
                  }
                  description="Vim key bindings when editing files. :w saves the active file."
                  control={<Switch id="settings-vim" aria-label="Vim mode" checked={vim} onCheckedChange={setVim} />}
                />
              </CardContent>
            </Card>
          )}
          {section === 'appearance' && (
            <Card>
              <CardHeader>
                <CardTitle>Appearance</CardTitle>
                <CardDescription>Colors across the app.</CardDescription>
              </CardHeader>
              <CardContent>
                <SettingRow
                  icon={<SunMoonIcon className="size-4" />}
                  label={
                    <Label htmlFor="settings-theme" className="text-sm font-medium">
                      Theme
                    </Label>
                  }
                  description="Dark+ is the default. System follows the OS appearance."
                  control={
                    <Select value={theme} onValueChange={(value) => setTheme(value as Theme)}>
                      <SelectTrigger id="settings-theme" aria-label="Theme" size="sm" className="w-44">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {THEME_OPTIONS.map((option) => (
                          <SelectItem key={option.id} value={option.id}>
                            <span className="flex flex-1 items-center justify-between gap-3">
                              <span>{option.label}</span>
                              <span className="flex items-center gap-1" aria-hidden>
                                {THEME_SWATCH[resolveTheme(option.id)].map((color) => (
                                  <span
                                    key={color}
                                    className="size-2.5 rounded-full border border-border"
                                    style={{ backgroundColor: color }}
                                  />
                                ))}
                              </span>
                            </span>
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  }
                />
              </CardContent>
            </Card>
          )}
          {section === 'acp-agents' && <AcpAgentsSection />}
        </div>
      </div>
    </section>
  )
}
