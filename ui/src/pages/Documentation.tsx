import { apiBase } from '@/api/base';
import { PageHeader, StatusBanner } from '@/components/Primitives';
import { useRuntimeInfo } from '@/hooks';

export function DocumentationPage() {
	const runtime = useRuntimeInfo();
	const path = runtime.data?.ui.documentationPath;

	if (runtime.isPending) {
		return <StatusBanner state="loading" title="Loading documentation" />;
	}
	if (!path) {
		return (
			<div className="documentation-unavailable">
				<PageHeader
					title="Documentation"
					actions={
						<a
							className="button"
							href="https://agentgateway.dev/docs/standalone/latest/"
							target="_blank"
							rel="noreferrer"
						>
							Open public documentation
						</a>
					}
				/>
			</div>
		);
	}
	return <iframe className="documentation-frame" title="Documentation" src={`${apiBase}${path}`} />;
}
