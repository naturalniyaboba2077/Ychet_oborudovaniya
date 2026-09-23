package ru.meshkeeper.app;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageInstaller;
import android.widget.Toast;

/**
 * Ответ системы на сессию установки из AppUpdater.
 *
 * Если системе нужно подтверждение человека (первое обновление, Android
 * до 12), она присылает сюда готовый экран — его и показываем. При успехе
 * процесс всё равно будет перезапущен, делать ничего не нужно.
 */
public class UpdateReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        if (!AppUpdater.ACTION_STATUS.equals(intent.getAction())) return;
        int status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE);
        if (status == PackageInstaller.STATUS_PENDING_USER_ACTION) {
            Intent confirm = intent.getParcelableExtra(Intent.EXTRA_INTENT);
            if (confirm != null) {
                confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                context.startActivity(confirm);
            }
        } else if (status != PackageInstaller.STATUS_SUCCESS
                && status != PackageInstaller.STATUS_FAILURE_ABORTED) {
            String msg = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE);
            Toast.makeText(context, "Обновление не установилось" + (msg == null ? "" : ": " + msg),
                    Toast.LENGTH_LONG).show();
        }
    }
}
