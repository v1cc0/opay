import { useLocation, useSearchParams as useReactRouterSearchParams } from 'react-router-dom';

export function useSearchParams(): URLSearchParams {
  const [searchParams] = useReactRouterSearchParams();
  return searchParams;
}

export function usePathname(): string {
  return useLocation().pathname;
}

export function redirect(to: string): never {
  if (typeof window !== 'undefined') {
    window.location.replace(to);
  }

  throw new Error(`redirect(${to}) called outside supported runtime`);
}
