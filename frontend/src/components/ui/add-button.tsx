import { Plus } from 'lucide-react'
import { Button, type ButtonProps } from '@/components/ui/button'

// Shared "add X" action button: a Plus icon followed by a label that is always
// visible (no responsive hiding). Use this anywhere the UI offers an add action
// so they stay visually consistent. Defaults to size="sm"; pass any Button prop
// (variant, onClick, disabled, …) through.
export function AddButton({ children, size = 'sm', ...props }: ButtonProps) {
  return (
    <Button size={size} {...props}>
      <Plus className="h-4 w-4" />
      {children}
    </Button>
  )
}
