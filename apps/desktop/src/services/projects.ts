import { call } from './call'
import type { Project } from './types'

export const projectService = {
  list: () => call<Project[]>('project_list'),
  add: (path: string) => call<Project>('project_add', { path }),
  remove: (id: string) => call<void>('project_remove', { id }),
}

export type ProjectService = typeof projectService
