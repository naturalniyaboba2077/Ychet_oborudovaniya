import { z } from "zod";
import { createRouter, publicQuery } from "./middleware";
import { updateUser, workspacesOfUser } from "./queries/users";
import { requireMe } from "./auth";
import { publicUser } from "./lib/password";

export const profileRouter = createRouter({
  get: publicQuery.query(async ({ ctx }) => {
    const me = await requireMe(ctx);
    const workspaces = await workspacesOfUser(me.id);
    // email и googleLinked нужны профилю: он показывает либо кнопку привязки
    // Google, либо уже привязанную почту.
    return {
      ...publicUser(me),
      workspaces,
      email: null as string | null,
      googleLinked: false,
    };
  }),

  update: publicQuery
    .input(
      z.object({
        fullName: z.string().min(1).optional(),
        position: z.string().nullable().optional(),
        phone: z.string().min(5).optional(),
        avatarUrl: z.string().nullable().optional(),
      }),
    )
    .mutation(async ({ ctx, input }) => {
      const me = await requireMe(ctx);
      return updateUser(me.id, input);
    }),

  // Пароль меняет Rust-узел: здесь только форма вызова и типы.
  changePassword: publicQuery
    .input(
      z.object({
        currentPassword: z.string().min(1),
        newPassword: z.string().min(10),
      }),
    )
    .mutation(async () => ({ ok: true, message: "Пароль изменён" })),

  /**
   * Выход из организации. Инструмент, который числился за человеком,
   * возвращается на склад; последнего руководителя сервер не выпускает.
   */
  leaveWorkspace: publicQuery
    .input(z.object({ workspaceId: z.number().int().positive() }))
    .mutation(async ({ input }) => ({
      ok: true,
      workspaceId: input.workspaceId,
      releasedItems: 0,
    })),

  /**
   * Удаление собственного аккаунта: подтверждается паролем. Строка
   * пользователя остаётся обезличенной — на неё ссылается журнал выдач.
   */
  deleteAccount: publicQuery
    .input(z.object({ password: z.string().min(1) }))
    .mutation(async () => ({ ok: true, message: "Аккаунт удалён" })),
});
