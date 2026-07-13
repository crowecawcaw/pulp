import React, { useEffect, useState } from 'react'
import { NavLink } from 'react-router-dom'
import {
  Home,
  Tag,
  Plug,
  Settings,
  ChevronDown,
  Plus,
} from 'lucide-react'
import { GrapefruitLogo } from '@/components/GrapefruitLogo'
import { useWorkspaces, useCreateWorkspace } from '@/api/queries'
import { useWorkspaceStore } from '@/stores/workspace'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useToast } from '@/components/ui/useToast'
import type { Workspace } from '@/api/types'

const navItems = [
  { to: '/feed',     icon: Home,     label: 'feed' },
  { to: '/monitors', icon: Tag,      label: 'monitors' },
  { to: '/channels', icon: Plug,     label: 'channels' },
  { to: '/settings', icon: Settings, label: 'settings' },
]

interface WorkspaceOptionsProps {
  workspaces: Workspace[]
  current: Workspace | null
  onSelectWorkspace: (ws: Workspace) => void
  onSetDropdownOpen: (open: boolean) => void
  onNew: () => void
}

function WorkspaceOptions({ workspaces, current, onSelectWorkspace, onSetDropdownOpen, onNew }: WorkspaceOptionsProps) {
  return (
    <>
      {workspaces.map((ws) => (
        <button
          key={ws.id}
          className={`ws-picker__opt${current?.id === ws.id ? ' ws-picker__opt--active' : ''}`}
          onClick={() => onSelectWorkspace(ws)}
        >
          {ws.name}
        </button>
      ))}
      <div className="ws-picker__divider">
        <button
          className="ws-picker__new"
          onClick={() => { onSetDropdownOpen(false); onNew() }}
        >
          <Plus className="h-3.5 w-3.5" />
          new workspace
        </button>
      </div>
    </>
  )
}

interface WorkspaceSelectorProps {
  current: Workspace | null
  dropdownOpen: boolean
  onSetDropdownOpen: (open: boolean) => void
  onNew: () => void
  workspaces: Workspace[]
  onSelectWorkspace: (ws: Workspace) => void
}

function WorkspaceSelector({ current, dropdownOpen, onSetDropdownOpen, onNew, workspaces, onSelectWorkspace }: WorkspaceSelectorProps) {
  return (
    <div className="ws-picker-wrap">
      <button className="ws-picker" onClick={() => onSetDropdownOpen(!dropdownOpen)}>
        <span className="ws-picker__name">{current ? current.name : 'select workspace'}</span>
        <ChevronDown className="h-4 w-4 ws-picker__caret" />
      </button>
      {dropdownOpen && (
        <div className="ws-picker__dropdown">
          <WorkspaceOptions workspaces={workspaces} current={current} onSelectWorkspace={onSelectWorkspace} onSetDropdownOpen={onSetDropdownOpen} onNew={onNew} />
        </div>
      )}
    </div>
  )
}

interface NavItemLinkProps {
  to: string
  icon: React.ElementType
  label: string
  onClick?: () => void
}

function NavItemLink({ to, icon: Icon, label, onClick }: NavItemLinkProps) {
  return (
    <NavLink
      to={to}
      onClick={onClick}
      className={({ isActive }) => `nav-item${isActive ? ' nav-item--active' : ''}`}
    >
      <Icon className="h-4 w-4" />
      {label}
    </NavLink>
  )
}

export default function Layout({ children }: { children: React.ReactNode }) {
  const { workspaces, current, setWorkspaces, setCurrent } = useWorkspaceStore()
  const { addToast } = useToast()
  const [dropdownOpen, setDropdownOpen] = useState(false)
  const [createDialogOpen, setCreateDialogOpen] = useState(false)
  const [newWsName, setNewWsName] = useState('')
  const [newWsDesc, setNewWsDesc] = useState('')

  // Workspaces come from React Query, but stay mirrored into the shared zustand
  // store so the rest of the app (pages that read `current`) is unaffected.
  const { data: workspaceData, isError } = useWorkspaces()
  const createWorkspace = useCreateWorkspace()

  useEffect(() => {
    if (!workspaceData) return
    setWorkspaces(workspaceData)
    if (workspaceData.length > 0 && !current) setCurrent(workspaceData[0])
  }, [workspaceData, current, setWorkspaces, setCurrent])

  useEffect(() => {
    if (isError) addToast('Failed to load workspaces', 'error')
  }, [isError, addToast])

  const creating = createWorkspace.isPending

  const handleCreateWorkspace = async () => {
    if (!newWsName.trim()) return
    try {
      const ws = await createWorkspace.mutateAsync({
        name: newWsName.trim(),
        description: newWsDesc.trim() || undefined,
      })
      setCurrent(ws)
      setCreateDialogOpen(false)
      setNewWsName('')
      setNewWsDesc('')
      addToast('Workspace created', 'success')
    } catch {
      addToast('Failed to create workspace', 'error')
    }
  }

  const handleSelectWorkspace = (ws: Workspace) => {
    setCurrent(ws)
    setDropdownOpen(false)
  }

  return (
    <div className="app-layout">
      {/* Desktop sidebar */}
      <aside className="hidden lg:flex sidebar">
        <div className="sidebar__logo">
          <GrapefruitLogo bare className="h-6 w-6 sidebar__logo-icon" />
          <span className="sidebar__logo-name">pulp</span>
        </div>
        <div className="sidebar__ws">
          <WorkspaceSelector
            current={current}
            dropdownOpen={dropdownOpen}
            onSetDropdownOpen={setDropdownOpen}
            onNew={() => setCreateDialogOpen(true)}
            workspaces={workspaces}
            onSelectWorkspace={handleSelectWorkspace}
          />
        </div>
        <nav className="sidebar__nav">
          {navItems.map(({ to, icon, label }) => (
            <NavItemLink key={to} to={to} icon={icon} label={label} />
          ))}
        </nav>
      </aside>

      {/* Right column */}
      <div className="app-col">
        {/* Mobile top bar */}
        <header className="lg:hidden topbar">
          <div className="topbar__brand">
            <GrapefruitLogo bare className="h-5 w-5 topbar__icon" />
            <span className="topbar__name">pulp</span>
          </div>
          <div className="topbar__ws ws-picker-wrap">
            <button className="topbar__menu" onClick={() => setDropdownOpen(!dropdownOpen)}>
              <span className="topbar__ws-name">{current?.name ?? 'select workspace'}</span>
              <ChevronDown className="h-5 w-5 ws-picker__caret" />
            </button>
            {dropdownOpen && (
              <>
                <div className="ws-picker__backdrop" onClick={() => setDropdownOpen(false)} />
                <div className="ws-picker__dropdown ws-picker__dropdown--topbar">
                  <WorkspaceOptions
                    workspaces={workspaces}
                    current={current}
                    onSelectWorkspace={handleSelectWorkspace}
                    onSetDropdownOpen={setDropdownOpen}
                    onNew={() => setCreateDialogOpen(true)}
                  />
                </div>
              </>
            )}
          </div>
        </header>

        {/* Main content */}
        <main className="app-content">
          {workspaces.length === 0 ? (
            <div className="center-fill">
              <p className="loading-text">no workspaces yet.</p>
              <Button onClick={() => setCreateDialogOpen(true)}>
                <Plus className="h-4 w-4" /> create your first workspace
              </Button>
            </div>
          ) : (
            children
          )}
        </main>

        {/* Mobile bottom nav */}
        <nav className="lg:hidden bottom-nav">
          <div className="bottom-nav__inner">
            {navItems.map(({ to, icon: Icon, label }) => (
              <NavLink
                key={to}
                to={to}
                className={({ isActive }) => `bottom-nav__item${isActive ? ' bottom-nav__item--active' : ''}`}
              >
                <Icon className="h-4 w-4" />
                <span>{label}</span>
              </NavLink>
            ))}
          </div>
        </nav>
      </div>

      {/* Create workspace dialog */}
      <Dialog open={createDialogOpen} onOpenChange={setCreateDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>new workspace</DialogTitle>
          </DialogHeader>
          <div className="dialog-form">
            <div className="field-group">
              <Label htmlFor="ws-name">name</Label>
              <Input
                id="ws-name"
                value={newWsName}
                onChange={(e) => setNewWsName(e.target.value)}
                placeholder="my brand"
              />
            </div>
            <div className="field-group">
              <Label htmlFor="ws-desc">description (optional)</Label>
              <Input
                id="ws-desc"
                value={newWsDesc}
                onChange={(e) => setNewWsDesc(e.target.value)}
                placeholder="what are you monitoring?"
              />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setCreateDialogOpen(false)}>cancel</Button>
            <Button onClick={handleCreateWorkspace} disabled={creating || !newWsName.trim()}>
              {creating ? 'creating...' : 'create'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
