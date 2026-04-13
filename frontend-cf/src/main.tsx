import React, { Suspense, lazy } from 'react';
import ReactDOM from 'react-dom/client';
import { BrowserRouter, Navigate, Route, Routes, useLocation } from 'react-router-dom';

import '@/app/globals.css';

const PayPage = lazy(() => import('@/app/pay/page'));
const OrdersPage = lazy(() => import('@/app/pay/orders/page'));
const PayResultPage = lazy(() => import('@/app/pay/result/page'));
const StripePopupPage = lazy(() => import('@/app/pay/stripe-popup/page'));
const AdminLayout = lazy(() => import('@/app/admin/layout'));
const AdminDashboardPage = lazy(() => import('@/app/admin/page'));
const DashboardPage = lazy(() => import('@/app/admin/dashboard/page'));
const AdminOrdersPage = lazy(() => import('@/app/admin/orders/page'));
const PaymentConfigPage = lazy(() => import('@/app/admin/payment-config/page'));
const ChannelsPage = lazy(() => import('@/app/admin/channels/page'));
const SubscriptionsPage = lazy(() => import('@/app/admin/subscriptions/page'));

function RootRedirect() {
  const location = useLocation();
  return <Navigate replace to={`/pay${location.search}`} />;
}

function AdminShell({ children }: { children: React.ReactNode }) {
  return <AdminLayout>{children}</AdminLayout>;
}

function RouteLoading() {
  return (
    <div className="flex min-h-screen items-center justify-center bg-slate-50 p-6 text-center text-slate-600">
      <div>
        <div className="text-sm font-medium text-slate-900">Loading...</div>
      </div>
    </div>
  );
}

function NotFoundPage() {
  return (
    <div className="flex min-h-screen items-center justify-center bg-slate-50 p-6 text-center text-slate-600">
      <div>
        <h1 className="text-2xl font-semibold text-slate-900">404</h1>
        <p className="mt-2">Page not found.</p>
      </div>
    </div>
  );
}

function App() {
  return (
    <BrowserRouter>
      <Suspense fallback={<RouteLoading />}>
        <Routes>
          <Route path="/" element={<RootRedirect />} />
          <Route path="/pay" element={<PayPage />} />
          <Route path="/pay/orders" element={<OrdersPage />} />
          <Route path="/pay/result" element={<PayResultPage />} />
          <Route path="/pay/stripe-popup" element={<StripePopupPage />} />

          <Route
            path="/admin"
            element={
              <AdminShell>
                <AdminDashboardPage />
              </AdminShell>
            }
          />
          <Route
            path="/admin/dashboard"
            element={
              <AdminShell>
                <DashboardPage />
              </AdminShell>
            }
          />
          <Route
            path="/admin/orders"
            element={
              <AdminShell>
                <AdminOrdersPage />
              </AdminShell>
            }
          />
          <Route
            path="/admin/payment-config"
            element={
              <AdminShell>
                <PaymentConfigPage />
              </AdminShell>
            }
          />
          <Route
            path="/admin/channels"
            element={
              <AdminShell>
                <ChannelsPage />
              </AdminShell>
            }
          />
          <Route
            path="/admin/subscriptions"
            element={
              <AdminShell>
                <SubscriptionsPage />
              </AdminShell>
            }
          />

          <Route path="*" element={<NotFoundPage />} />
        </Routes>
      </Suspense>
    </BrowserRouter>
  );
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
