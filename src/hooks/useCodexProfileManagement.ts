import { useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { CODEX_DEFAULT_PROFILE_ID } from "@/config/constants";
import { codexProfilesApi } from "@/lib/api/codexProfiles";
import {
  codexProfileKeys,
  useCreateCodexProfile,
  useDeleteCodexProfile,
  useUpdateCodexProfile,
} from "@/lib/query/codexProfiles";
import type {
  CodexProfile,
  CodexProfileState,
  CreateCodexProfileInput,
  UpdateCodexProfileInput,
} from "@/types/codexProfile";

interface UseCodexProfileManagementOptions {
  profiles: CodexProfile[];
  selectedProfileId: string | null;
  onSelectProfile: (profileId: string) => void;
}

/** 从删除后的列表中优先选择默认 Profile，否则选择第一项。 */
function findSelectionAfterDelete(
  profiles: CodexProfile[],
  deletedProfileId: string,
): CodexProfile | undefined {
  const remainingProfiles = profiles.filter(
    (profile) => profile.id !== deletedProfileId,
  );
  return (
    remainingProfiles.find(
      (profile) => profile.id === CODEX_DEFAULT_PROFILE_ID,
    ) ?? remainingProfiles[0]
  );
}

/** 协调 Profile mutation、选择规则与按 Profile 隔离的状态加载。 */
export function useCodexProfileManagement({
  profiles,
  selectedProfileId,
  onSelectProfile,
}: UseCodexProfileManagementOptions) {
  const queryClient = useQueryClient();
  const createMutation = useCreateCodexProfile();
  const updateMutation = useUpdateCodexProfile();
  const deleteMutation = useDeleteCodexProfile();

  /** 创建 Profile，并在成功后将它设为当前 Profile。 */
  const createProfile = useCallback(
    async (input: CreateCodexProfileInput): Promise<CodexProfile> => {
      const createdProfile = await createMutation.mutateAsync(input);
      onSelectProfile(createdProfile.id);
      return createdProfile;
    },
    [createMutation, onSelectProfile],
  );

  /** 使用单次原子 mutation 更新 Profile。 */
  const updateProfile = useCallback(
    (input: UpdateCodexProfileInput): Promise<CodexProfile> =>
      updateMutation.mutateAsync(input),
    [updateMutation],
  );

  /** 删除 Profile，并只在删除当前项时选择安全回退项。 */
  const deleteProfile = useCallback(
    async (profileId: string): Promise<boolean> => {
      const deleted = await deleteMutation.mutateAsync(profileId);
      if (deleted && profileId === selectedProfileId) {
        const fallbackProfile = findSelectionAfterDelete(profiles, profileId);
        if (fallbackProfile) {
          onSelectProfile(fallbackProfile.id);
        }
      }
      return deleted;
    },
    [deleteMutation, onSelectProfile, profiles, selectedProfileId],
  );

  /** 通过携带 Profile ID 的查询键读取目标运行时状态。 */
  const loadProfileState = useCallback(
    (profileId: string): Promise<CodexProfileState> =>
      queryClient.fetchQuery({
        queryKey: codexProfileKeys.state(profileId),
        queryFn: () => codexProfilesApi.getState(profileId),
      }),
    [queryClient],
  );

  return {
    createProfile,
    updateProfile,
    deleteProfile,
    loadProfileState,
  };
}
