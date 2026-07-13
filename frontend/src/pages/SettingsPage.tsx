import { useEffect, useReducer } from 'react'
import { useAiConfig, useUpdateAiConfig, useTestAiConfig } from '@/api/queries'
import { useToast } from '@/components/ui/useToast'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { ToggleSwitch } from '@/components/ui/toggle-switch'
import { NotificationsSettings } from '@/components/NotificationsSettings'
import type { AiConfigView, AiConfigUpdate, AiTestResult } from '@/api/types'

interface FormState {
  enabled: boolean
  baseUrl: string
  model: string
  apiKey: string
  apiKeySet: boolean
  clearKey: boolean
  testResult: AiTestResult | null
}

type FormAction =
  | { type: 'setEnabled'; payload: boolean }
  | { type: 'setBaseUrl'; payload: string }
  | { type: 'setModel'; payload: string }
  | { type: 'setApiKey'; payload: string }
  | { type: 'setApiKeySet'; payload: boolean }
  | { type: 'setClearKey'; payload: boolean }
  | { type: 'setTestResult'; payload: AiTestResult | null }
  | { type: 'loadConfig'; payload: AiConfigView }

function formReducer(state: FormState, action: FormAction): FormState {
  switch (action.type) {
    case 'setEnabled':
      return { ...state, enabled: action.payload }
    case 'setBaseUrl':
      return { ...state, baseUrl: action.payload }
    case 'setModel':
      return { ...state, model: action.payload }
    case 'setApiKey':
      return { ...state, apiKey: action.payload }
    case 'setApiKeySet':
      return { ...state, apiKeySet: action.payload }
    case 'setClearKey':
      return { ...state, clearKey: action.payload }
    case 'setTestResult':
      return { ...state, testResult: action.payload }
    case 'loadConfig':
      return {
        ...state,
        enabled: action.payload.enabled,
        baseUrl: action.payload.base_url,
        model: action.payload.model,
        apiKeySet: action.payload.api_key_set,
        apiKey: '',
        clearKey: false,
      }
    default:
      return state
  }
}

export default function SettingsPage() {
  const { addToast } = useToast()

  const { data: aiConfig, isLoading: loading, isError } = useAiConfig()
  const updateAi = useUpdateAiConfig()
  const testAi = useTestAiConfig()
  const saving = updateAi.isPending
  const testing = testAi.isPending

  const [state, dispatch] = useReducer(formReducer, {
    enabled: false,
    baseUrl: '',
    model: '',
    apiKey: '',
    apiKeySet: false,
    clearKey: false,
    testResult: null,
  })

  const { enabled, baseUrl, model, apiKey, apiKeySet, clearKey, testResult } = state

  const applyView = (cfg: AiConfigView) => {
    dispatch({ type: 'loadConfig', payload: cfg })
  }

  // Seed the edit form from the fetched config once it arrives.
  useEffect(() => {
    if (aiConfig) dispatch({ type: 'loadConfig', payload: aiConfig })
  }, [aiConfig])

  useEffect(() => {
    if (isError) addToast('Failed to load AI settings', 'error')
  }, [isError, addToast])

  const handleSave = async () => {
    if (enabled && !baseUrl.trim()) {
      addToast('A base URL is required to enable AI filtering', 'error')
      return
    }
    if (enabled && !model.trim()) {
      addToast('A model is required to enable AI filtering', 'error')
      return
    }
    dispatch({ type: 'setTestResult', payload: null })
    try {
      const body: AiConfigUpdate = {
        enabled,
        base_url: baseUrl.trim(),
        model: model.trim(),
        api_key: apiKey.trim() !== '' ? apiKey.trim() : clearKey ? '' : null,
      }
      const updated = await updateAi.mutateAsync(body)
      applyView(updated)
      addToast('AI settings saved', 'success')
    } catch (e) {
      addToast(e instanceof Error ? e.message : 'Failed to save AI settings', 'error')
    }
  }

  const handleTest = async () => {
    dispatch({ type: 'setTestResult', payload: null })
    try {
      const res = await testAi.mutateAsync()
      dispatch({ type: 'setTestResult', payload: res })
    } catch (e) {
      dispatch({ type: 'setTestResult', payload: { ok: false, error: e instanceof Error ? e.message : 'Request failed' } })
    }
  }

  if (loading) {
    return (
      <div className="page-narrow">
        <div className="page-hd">
          <h1 className="page-title">settings</h1>
        </div>
        <div className="loading-text">loading...</div>
      </div>
    )
  }

  return (
    <div className="page-narrow">
      <div className="page-hd">
        <h1 className="page-title">settings</h1>
      </div>

      <section className="settings-section">
        <h2 className="settings-section__title">notifications</h2>
        <p className="settings-section__desc">
          everything that reaches this workspace's feed is delivered here.
        </p>
        <NotificationsSettings />
      </section>

      <section className="settings-section">
        <h2 className="settings-section__title">AI relevance filter</h2>
        <p className="settings-section__desc">
          optional — bring your own OpenAI-compatible LLM endpoint.
        </p>

        <div className="settings-card">
          <div className="settings-row">
            <div>
              <p className="settings-row__title">enabled</p>
              <p className="settings-row__desc">
                when on, monitors with an AI prompt have new mentions judged for relevance before they reach the feed.
              </p>
            </div>
            <ToggleSwitch checked={enabled} onChange={() => dispatch({ type: 'setEnabled', payload: !enabled })} />
          </div>

        <div className="field-group">
          <Label htmlFor="base-url">base URL</Label>
          <Input
            id="base-url"
            value={baseUrl}
            onChange={(e) => dispatch({ type: 'setBaseUrl', payload: e.target.value })}
            placeholder="http://localhost:11434/v1"
          />
          <p className="field-hint">
            any OpenAI-compatible endpoint — Ollama (<code>/v1</code>), LM Studio, llama-server, vLLM, OpenAI, OpenRouter… the <code>/chat/completions</code> path is appended automatically.
          </p>
        </div>

        <div className="field-group">
          <Label htmlFor="model">model</Label>
          <Input
            id="model"
            value={model}
            onChange={(e) => dispatch({ type: 'setModel', payload: e.target.value })}
            placeholder="llama3.2"
          />
        </div>

        <div className="field-group">
          <Label htmlFor="api-key">API key</Label>
          <Input
            id="api-key"
            type="password"
            value={apiKey}
            onChange={(e) => {
              dispatch({ type: 'setApiKey', payload: e.target.value })
              dispatch({ type: 'setClearKey', payload: false })
            }}
            placeholder={apiKeySet ? '•••••••• (set — leave blank to keep)' : 'optional — for hosted providers'}
          />
          {apiKeySet && apiKey.trim() === '' && (
            <label className="field-check field-check--sm">
              <input type="checkbox" checked={clearKey} onChange={(e) => dispatch({ type: 'setClearKey', payload: e.target.checked })} />
              remove the stored API key
            </label>
          )}
        </div>

        <div className="settings-btn-row">
          <Button onClick={handleSave} disabled={saving}>
            {saving ? 'saving...' : 'save'}
          </Button>
          <Button variant="outline" onClick={handleTest} disabled={testing}>
            {testing ? 'testing...' : 'test connection'}
          </Button>
          <span className="field-hint">test uses the saved settings.</span>
        </div>

        {testResult && (
          <div className={`test-result${testResult.ok ? ' test-result--ok' : ' test-result--err'}`}>
            {testResult.ok ? (
              <span className="test-result__ok">
                connection OK.{' '}
                <span className="test-result__detail">
                  sample verdict: <code>{testResult.verdict ?? '?'}</code>
                  {testResult.reason ? ` — ${testResult.reason}` : ''}
                </span>
              </span>
            ) : (
              <span className="test-result__err">
                test failed.{' '}
                <span className="test-result__detail">{testResult.error ?? 'unknown error'}</span>
              </span>
            )}
          </div>
        )}
        </div>
      </section>
    </div>
  )
}
