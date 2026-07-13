import { create } from 'zustand'
import type { Workspace } from '@/api/types'

interface WorkspaceState {
  workspaces: Workspace[]
  current: Workspace | null
  setWorkspaces: (ws: Workspace[]) => void
  setCurrent: (ws: Workspace | null) => void
}

export const useWorkspaceStore = create<WorkspaceState>((set) => ({
  workspaces: [],
  current: null,
  setWorkspaces: (workspaces) => set({ workspaces }),
  setCurrent: (current) => set({ current }),
}))
