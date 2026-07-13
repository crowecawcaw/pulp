import type React from 'react'
import { useEffect, useState } from 'react'
import { Bell, Webhook, Send, Trash2, Smartphone } from 'lucide-react'
import {
  useNotifications,
  useCreateNotification,
  useDeleteNotification,
  useTestNotifications,
} from '@/api/queries'
import {
  enablePush,
  currentPushEndpoint,
  deviceLabel,
  pushSupported,
  isStandalone,
  isIos,
} from '@/lib/push'
import { useWorkspaceStore } from '@/stores/workspace'
import { useToast } from '@/components/ui/useToast'
import { Button } from '@/components/ui/button'
import { AddButton } from '@/components/ui/add-button'
import { Input } from '@/components/ui/input'
import type { Notification } from '@/api/types'

const KIND_ICONS: Record<string, React.ElementType> = {
  webpush: Smartphone,
  webhook: Webhook,
}

const KIND_LABELS: Record<string, string> = {
  webpush: 'web push',
  webhook: 'webhook',
}

// A readable name for a notification row: its label if set, otherwise something
// derived from its kind/config (the webhook host, or a generic device).
function notificationName(n: Notification): string {
  if (n.label) return n.label
  if (n.kind === 'webhook') {
    const url = typeof n.config.url === 'string' ? n.config.url : ''
    try {
      return url ? new URL(url).host : 'webhook'
    } catch {
      return url || 'webhook'
    }
  }
  return 'web push device'
}

// The notifications settings block: the workspace's delivery destinations plus
// the controls to add more. Rendered as a card inside the Settings page.
export function NotificationsSettings() {
  const { current } = useWorkspaceStore()
  const { addToast } = useToast()
  const {
    data: notifications = [],
    isLoading,
    isError,
  } = useNotifications(current?.id)
  const createNotification = useCreateNotification(current?.id)
  const deleteNotification = useDeleteNotification(current?.id)
  const testNotifications = useTestNotifications(current?.id)

  const [webhookUrl, setWebhookUrl] = useState('')
  const [showWebhook, setShowWebhook] = useState(false)
  const [enabling, setEnabling] = useState(false)

  // The push endpoint this browser is currently subscribed to, so we can detect
  // whether the current device is already enabled on this workspace.
  const [deviceEndpoint, setDeviceEndpoint] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    currentPushEndpoint().then((ep) => { if (alive) setDeviceEndpoint(ep) })
    return () => { alive = false }
  }, [])

  useEffect(() => {
    if (isError) addToast('Failed to load notifications', 'error')
  }, [isError, addToast])

  // Is THIS device already a webpush notification in this workspace?
  const thisDeviceNotification = deviceEndpoint
    ? notifications.find((n) => n.kind === 'webpush' && n.config.endpoint === deviceEndpoint)
    : undefined

  const webpushBlocked = !pushSupported() || (isIos() && !isStandalone())

  const handleEnableDevice = async () => {
    if (!current) return
    setEnabling(true)
    try {
      const sub = await enablePush()
      await createNotification.mutateAsync({
        workspace_id: current.id,
        kind: 'webpush',
        config: { ...sub },
        label: deviceLabel(),
      })
      setDeviceEndpoint(sub.endpoint)
      addToast('Notifications enabled on this device', 'success')
    } catch (e) {
      addToast(e instanceof Error ? e.message : 'Failed to enable notifications', 'error')
    } finally {
      setEnabling(false)
    }
  }

  const handleAddWebhook = async () => {
    if (!current || !webhookUrl.trim()) return
    try {
      await createNotification.mutateAsync({
        workspace_id: current.id,
        kind: 'webhook',
        config: { url: webhookUrl.trim() },
      })
      setWebhookUrl('')
      setShowWebhook(false)
      addToast('Webhook added', 'success')
    } catch (e) {
      addToast(e instanceof Error ? e.message : 'Failed to add webhook', 'error')
    }
  }

  const handleDelete = async (n: Notification) => {
    try {
      await deleteNotification.mutateAsync(n.id)
      addToast('Notification removed', 'success')
    } catch {
      addToast('Failed to remove notification', 'error')
    }
  }

  const handleSendTest = async () => {
    try {
      const { delivered } = await testNotifications.mutateAsync()
      addToast(
        delivered > 0
          ? `test sent to ${delivered} notification${delivered === 1 ? '' : 's'}`
          : 'no notifications configured yet',
        delivered > 0 ? 'success' : 'error',
      )
    } catch (e) {
      addToast(e instanceof Error ? e.message : 'Failed to send test', 'error')
    }
  }

  const sendingTest = testNotifications.isPending

  if (!current) return <div className="page-empty">select a workspace first.</div>

  return (
    <div className="settings-card">
      {isLoading ? (
        <div className="loading-text">loading...</div>
      ) : (
        <>
          {notifications.length === 0 ? (
            <div className="empty-state">no notifications yet.</div>
          ) : (
            <div className="dest-list">
              {notifications.map((n) => {
                const Icon = KIND_ICONS[n.kind] ?? Bell
                const isThisDevice = n.kind === 'webpush' && n.config.endpoint === deviceEndpoint
                return (
                  <div key={n.id} className="dest-row">
                    <div className="dest-row__info">
                      <Icon className="h-4 w-4 dest-row__icon" />
                      <span>{notificationName(n)}</span>
                      <span className="dest-row__freq">
                        ({KIND_LABELS[n.kind] ?? n.kind}{isThisDevice ? ', this device' : ''})
                      </span>
                    </div>
                    <div className="dest-row__actions">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="btn--del"
                        onClick={() => handleDelete(n)}
                        title="Remove this notification"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </Button>
                    </div>
                  </div>
                )
              })}
            </div>
          )}

          <div className="stack--sm">
            {thisDeviceNotification ? (
              <p className="settings-row__desc">
                this device is enabled for web push.
              </p>
            ) : webpushBlocked ? (
              isIos() && !isStandalone() ? (
                <p className="settings-row__desc">
                  add Pulp to your Home Screen first to enable web push on iPhone &amp; iPad.
                </p>
              ) : (
                <p className="settings-row__desc settings-row__desc--error">
                  this browser doesn't support web push notifications.
                </p>
              )
            ) : (
              <Button onClick={handleEnableDevice} disabled={enabling}>
                <Bell className="h-4 w-4" />
                {enabling ? 'enabling...' : 'enable on this device'}
              </Button>
            )}

            {showWebhook ? (
              <div className="row--3">
                <Input
                  value={webhookUrl}
                  onChange={(e) => setWebhookUrl(e.target.value)}
                  placeholder="https://example.com/hook"
                  onKeyDown={(e) => { if (e.key === 'Enter') handleAddWebhook() }}
                />
                <Button
                  size="sm"
                  onClick={handleAddWebhook}
                  disabled={createNotification.isPending || !webhookUrl.trim()}
                >
                  add
                </Button>
                <Button variant="ghost" size="sm" onClick={() => { setShowWebhook(false); setWebhookUrl('') }}>
                  cancel
                </Button>
              </div>
            ) : (
              <AddButton variant="outline" onClick={() => setShowWebhook(true)}>
                add webhook
              </AddButton>
            )}
          </div>

          {notifications.length > 0 && (
            <div className="settings-btn-row">
              <Button variant="outline" size="sm" onClick={handleSendTest} disabled={sendingTest}>
                <Send className="h-3.5 w-3.5" />
                {sendingTest ? 'sending...' : 'send test'}
              </Button>
            </div>
          )}
        </>
      )}
    </div>
  )
}
