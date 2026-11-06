import { call } from './call'

export type Activity = 'working' | 'waiting' | 'done'
export type ActivityMap = Record<string, Activity>

export const activityService = {
  get: () => call<ActivityMap>('activity_get'),
}
