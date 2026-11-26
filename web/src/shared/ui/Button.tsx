/**
 * The only button in the app. Pages must not roll their own (design §7.10.2:
 * "页面不得自带主题/弹窗/Toast 实现").
 */
import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from 'react'

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: 'primary' | 'default' | 'ghost' | 'danger'
  size?: 'sm' | 'md'
  block?: boolean
  children?: ReactNode
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button({
  variant = 'default',
  size = 'md',
  block = false,
  className,
  type = 'button',
  ...rest
}, ref): JSX.Element {
  const classes = ['ark-btn', `ark-btn--${variant}`, `ark-btn--${size}`]
  if (block) classes.push('ark-btn--block')
  if (className) classes.push(className)
  return <button ref={ref} type={type} className={classes.join(' ')} {...rest} />
})
