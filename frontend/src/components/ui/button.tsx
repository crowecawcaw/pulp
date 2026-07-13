import * as React from 'react'
import { Slot } from '@radix-ui/react-slot'
import { cn } from '@/lib/utils'

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'default' | 'outline' | 'ghost' | 'destructive' | 'secondary' | 'link'
  size?: 'default' | 'sm' | 'lg' | 'icon'
  asChild?: boolean
}

const variantClass: Record<string, string> = {
  default:     '',
  outline:     'btn--outline',
  ghost:       'btn--ghost',
  destructive: 'btn--destructive',
  secondary:   '',
  link:        'btn--ghost',
}

const sizeClass: Record<string, string> = {
  default: '',
  sm:      'btn--sm',
  lg:      '',
  icon:    'btn--sm',
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant = 'default', size = 'default', asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : 'button'
    return (
      <Comp
        ref={ref}
        className={cn('btn', variantClass[variant], sizeClass[size], className)}
        {...props}
      />
    )
  }
)
Button.displayName = 'Button'

export { Button }
