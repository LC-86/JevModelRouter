export type CpaStage = 'idle'|'starting'|'waiting'|'connected'|'failed'|'cancelled'|'disconnected';
export interface CpaView {
  provider_id:string; provider:string; connection_name:string; connection_instance_id:string; generation:number;
  stage:CpaStage; account:string|null; plan:string|null; credential_reference?:string|null;
  catalog_state:string; observed_at:string|null; error:string|null;
  authorization_url:string|null; service_available:boolean;
  qualification:string; quota:string; capability:string;
  owned_service?:{running:boolean;pid:number;port:number;binary:string;artifact_sha256:string;evidence_file?:string}|null;
  hand_run?:{enabled:boolean;stopped:boolean;used:number;max_requests:number;expires_at:number;model_id:string}|null;
  models:{id:string;model_id:string;name:string;selected:boolean;bound:boolean}[];
}

export const cpaStageLabel:Record<CpaStage,string> = {
  idle:'未连接', starting:'发起授权', waiting:'等待授权', connected:'已连接', failed:'授权失败', cancelled:'已取消', disconnected:'已退出',
};
