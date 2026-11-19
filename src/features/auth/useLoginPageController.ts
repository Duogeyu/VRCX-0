import { useMemo } from 'react';
import { useShallow } from 'zustand/react/shallow';

import { resolveServerHealth } from '@/lib/serverHealth';
import { links } from '@/shared/constants/link';
import { useRuntimeStore } from '@/state/runtimeStore';
import { useVrccHealthStore } from '@/state/vrccHealthStore';

import { useLoginPageState } from './useLoginPageState';

export function useLoginPageController() {
    const page = useLoginPageState();
    // 取判定所需的全部字段，交给 `@/lib/serverHealth` 出结论。
    //
    // 刻意连 `error` 和 `lastFetchedAt` 一起取：登录页是最需要区分"服务挂了"和
    // "我连不上"的地方，而这两个字段正是那个区分的依据。早先这里只取
    // indicator/status/summary，于是刷新失败时登录页什么都不说。
    const serverStatus = useRuntimeStore(
        useShallow((state) => ({
            indicator: state.vrcStatus.indicator,
            status: state.vrcStatus.status,
            summary: state.vrcStatus.summary,
            error: state.vrcStatus.error,
            lastFetchedAt: state.vrcStatus.lastFetchedAt
        }))
    );
    const health = useMemo(
        () => resolveServerHealth(serverStatus),
        [serverStatus]
    );

    // 星遇同频那边的结论：由 `VrccHealthProbeHost` 常驻探测写进 store，
    // 这里只读。两条探测互相独立 —— 一个是 statuspage 的分级，一个是我们
    // 自家的可达性，合并不了也不该合并。
    const vrccHealth = useVrccHealthStore((state) => state.health);

    return {
        actions: {
            openDiscord: () => page.openExternalLink(links.discord),
            openForgotPassword: () =>
                page.openExternalLink(links.vrchatPassword),
            openGithub: () => page.openExternalLink(links.github),
            openRegister: () => page.openExternalLink(links.vrchatRegister)
        },
        deleteDialog: {
            deleteTarget: page.deleteTarget,
            isDeleting: page.isDeleting,
            onConfirm: page.handleDeleteSavedAccount,
            onOpenChange: (open: boolean) => {
                if (!open) {
                    page.setDeleteTarget(null);
                }
            }
        },
        form: {
            busy: page.isAuthBusy,
            loginErrors: page.loginErrors,
            loginForm: page.loginForm,
            onCancelAutoLogin: page.cancelPendingAutoLogin,
            onClearPreparedAccount: page.clearPreparedAccount,
            onPrepareSavedAccount: page.prepareSavedAccountLogin,
            onStartQrLogin: page.startQrLogin,
            onSubmit: page.handleManualLoginSubmit,
            preparedAccount: page.preparedAccount,
            setLoginErrors: page.setLoginErrors,
            setLoginForm: page.setLoginForm,
            submitting: page.isSubmitting
        },
        qrLogin: {
            onCancel: page.cancelQrLogin,
            onRetry: page.startQrLogin,
            onUsePassword: page.closeQrLogin,
            pending: page.pendingDeviceLogin,
            view: page.qrLogin
        },
        header: {
            locale: page.locale,
            onLanguageChange: page.handleLanguageChange
        },
        utilities: {
            disabled: page.isAuthBusy,
            isValidatingRestore: page.isValidatingRestore,
            onMigrateLegacyVrcxData: page.migrateLegacyVrcxData,
            onOpenProxyDialog: page.openProxyDialog,
            onRestoreProfileBackup: page.restoreProfileBackup,
            showLegacyMigration: page.showLegacyMigrationAction
        },
        proxyDialog: {
            enabled: page.proxyEnabledInput,
            isSaving: page.isSavingProxySettings,
            isTesting: page.isTestingProxySettings,
            onOpenChange: page.setIsProxyDialogOpen,
            onProxyEnabledChange: page.setProxyEnabledInput,
            onProxyInputChange: page.setProxyInput,
            onSave: () => page.saveProxySettings(false),
            onSaveAndRestart: () => page.saveProxySettings(true),
            onTest: page.testProxySettings,
            open: page.isProxyDialogOpen,
            proxyInput: page.proxyInput
        },
        savedAccounts: {
            accounts: page.savedAccounts,
            activeSavedUserId: page.activeSavedUserId,
            isAuthBusy: page.isAuthBusy,
            isDeleting: page.isDeleting,
            onCancelAutoLogin: page.cancelPendingAutoLogin,
            onDeleteStart: page.setDeleteTarget,
            onLogin: page.handleSavedCredentialLogin,
            visible: page.hasSavedAccounts
        },
        serverStatus: {
            health,
            onOpenStatusPage: () => page.openExternalLink(links.vrchatStatus)
        },
        vrccStatus: {
            health: vrccHealth,
            onOpenStatusPage: () => page.openExternalLink(links.vrccStatus)
        }
    };
}
