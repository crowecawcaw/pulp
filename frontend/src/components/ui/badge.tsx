import * as React from 'react'
import { cn } from '@/lib/utils'

export interface BadgeProps extends React.HTMLAttributes<HTMLSpanElement> {
  variant?: 'default' | 'secondary' | 'success' | 'warning' | 'destructive' | 'muted' | 'outline'
}

const variantClass: Record<string, string> = {
  default:     'badge--default',
  secondary:   'badge--secondary',
  success:     'badge--success',
  warning:     'badge--warning',
  destructive: 'badge--destructive',
  muted:       'badge--muted',
  outline:     'badge--outline',
}

function Badge({ className, variant = 'default', ...props }: BadgeProps) {
  return <span className={cn('badge', variantClass[variant], className)} {...props} />
}

export { Badge }
