// Shared on/off switch used across Settings, Channels, and Monitors — a
// single small control (not the shadcn "Toggle" family, hence the distinct
// file/component name) so its markup/behavior can't drift between pages.
export function ToggleSwitch({ checked, onChange }: { checked: boolean; onChange: () => void }) {
  return (
    <button
      type="button"
      onClick={onChange}
      className={`toggle${checked ? ' toggle--on' : ' toggle--off'}`}
      aria-checked={checked}
      role="switch"
    >
      <span className="toggle__knob" />
    </button>
  )
}
