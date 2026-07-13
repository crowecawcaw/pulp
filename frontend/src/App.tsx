import { Routes, Route, Navigate } from 'react-router-dom'
import Layout from '@/components/Layout'
import FeedPage from '@/pages/FeedPage'
import MentionDetailPage from '@/pages/MentionDetailPage'
import MonitorsPage from '@/pages/MonitorsPage'
import ChannelsPage from '@/pages/ChannelsPage'
import ChannelDetailPage from '@/pages/ChannelDetailPage'
import SettingsPage from '@/pages/SettingsPage'

export default function App() {
  return (
    <Routes>
      <Route path="/*" element={
        <Layout>
          <Routes>
            <Route index element={<Navigate to="/feed" replace />} />
            <Route path="feed" element={<FeedPage />} />
            <Route path="mentions/:id" element={<MentionDetailPage />} />
            <Route path="monitors" element={<MonitorsPage />} />
            <Route path="channels" element={<ChannelsPage />} />
            <Route path="channels/:channel" element={<ChannelDetailPage />} />
            <Route path="settings" element={<SettingsPage />} />
          </Routes>
        </Layout>
      } />
    </Routes>
  )
}
