package ru.meshkeeper.app;

import android.Manifest;
import android.annotation.SuppressLint;
import android.app.Activity;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.view.View;
import android.webkit.PermissionRequest;
import android.webkit.CookieManager;
import android.webkit.ValueCallback;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.EditText;
import android.widget.TextView;
import android.widget.Toast;

import java.util.Arrays;

import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.annotation.NonNull;
import androidx.appcompat.app.AppCompatActivity;
import androidx.core.app.ActivityCompat;
import androidx.core.content.ContextCompat;

import com.journeyapps.barcodescanner.ScanContract;
import com.journeyapps.barcodescanner.ScanOptions;


public class MainActivity extends AppCompatActivity {
    private static final String PREFS = "meshkeeper";
    private static final String KEY_RELAY = "relay";

    /**
     * Адрес сервера, с которым приложение работает по умолчанию.
     *
     * Раньше приложение начинало со своего экрана настройки: человек видел
     * не то, что на сайте, и должен был откуда-то знать адрес. Теперь оно
     * открывает тот же интерфейс, что и браузер. Экран настройки остался —
     * на случай другого сервера, — но по умолчанию не показывается.
     */
    private static final String DEFAULT_RELAY = "https://31-172-72-140.sslip.io";

    private WebView web;
    private View setup;
    private EditText serverUrl;
    private TextView lanHint;
    private String pendingMode = "join";
    /** Адрес сервера, с которого открыт интерфейс. Пустой — интерфейс не загружен. */
    private String serverOrigin = "";
    private PermissionRequest pendingWebPermission;
    private ValueCallback<Uri[]> fileCallback;

    private final ActivityResultLauncher<ScanOptions> qrLauncher = registerForActivityResult(
            new ScanContract(),
            result -> {
                if (result == null || result.getContents() == null) return;
                String code = result.getContents();
                web.evaluateJavascript(
                        "window.__onNativeQr && window.__onNativeQr(" + org.json.JSONObject.quote(code) + ");",
                        null);
            });

    private final ActivityResultLauncher<Intent> fileLauncher = registerForActivityResult(
            new ActivityResultContracts.StartActivityForResult(),
            result -> {
                Uri[] uris = WebChromeClient.FileChooserParams.parseResult(
                        result.getResultCode() == Activity.RESULT_OK ? Activity.RESULT_OK : result.getResultCode(),
                        result.getData());
                if (fileCallback != null) {
                    fileCallback.onReceiveValue(uris);
                    fileCallback = null;
                }
            });

    @SuppressLint({"SetJavaScriptEnabled", "AddJavascriptInterface"})
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_main);
        web = findViewById(R.id.web);
        setup = findViewById(R.id.setup);
        serverUrl = findViewById(R.id.serverUrl);
        lanHint = findViewById(R.id.lanHint);
        Button btnJoin = findViewById(R.id.btnJoin);
        Button btnCreate = findViewById(R.id.btnCreate);
        Button btnLogin = findViewById(R.id.btnLogin);

        SharedPreferences prefs = getSharedPreferences(PREFS, MODE_PRIVATE);
        String savedRelay = prefs.getString(KEY_RELAY, DEFAULT_RELAY);
        serverUrl.setText(savedRelay);

        // Открываемся сразу интерфейсом, как в браузере. Экран настройки
        // покажется, только если адрес не задан или страница не загрузилась.
        WebSettings s = web.getSettings();
        s.setJavaScriptEnabled(true);
        s.setDomStorageEnabled(true);
        s.setDatabaseEnabled(true);
        s.setMediaPlaybackRequiresUserGesture(true);
        s.setAllowFileAccess(false);
        s.setAllowContentAccess(false);
        s.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        s.setJavaScriptCanOpenWindowsAutomatically(false);
        s.setSupportMultipleWindows(false);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            s.setSafeBrowsingEnabled(true);
        }
        s.setUserAgentString(s.getUserAgentString() + " MeshKeeperAndroid");
        WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG);
        CookieManager.getInstance().setAcceptCookie(true);
        // Чужие cookie не принимаем: приложению нужен только свой сервер.
        // Страницы входа Google работают на своих доменах и своих cookie,
        // третьей стороной для нас не являются.
        CookieManager.getInstance().setAcceptThirdPartyCookies(web, false);
        web.setLayerType(View.LAYER_TYPE_HARDWARE, null);
        web.addJavascriptInterface(new JsBridge(), "MeshKeeperNative");
        web.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                Uri target = request.getUrl();
                if (isTrustedLocalOrigin(target)) return false;
                // Вход через Google должен идти внутри приложения. Если
                // отправить его во внешний браузер, сессия достанется браузеру:
                // cookie лягут туда, а приложение останется незалогиненным.
                if (isGoogleSignIn(target)) return false;
                if ("https".equalsIgnoreCase(target.getScheme())) {
                    try {
                        startActivity(new Intent(Intent.ACTION_VIEW, target));
                    } catch (Exception ignored) {
                        Toast.makeText(MainActivity.this, "Не удалось открыть ссылку", Toast.LENGTH_SHORT).show();
                    }
                }
                return true;
            }

            @Override
            public void onReceivedError(WebView view, WebResourceRequest request,
                                        android.webkit.WebResourceError error) {
                // Сервер недоступен или адрес неверный: показываем настройку,
                // а не белый экран, на котором человеку нечего делать.
                if (request.isForMainFrame()) {
                    web.setVisibility(View.GONE);
                    setup.setVisibility(View.VISIBLE);
                    showSetupHint();
                    Toast.makeText(MainActivity.this,
                            "Не удалось открыть сервер. Проверьте адрес и связь.",
                            Toast.LENGTH_LONG).show();
                }
            }

            @Override
            public void onPageFinished(WebView view, String url) {
                // Раньше сюда подставлялся LAN-адрес телефона-узла; теперь
                // приглашения ведут на сервер, и подставлять нечего.
            }
        });
        web.setWebChromeClient(new WebChromeClient() {
            @Override
            public void onPermissionRequest(PermissionRequest request) {
                runOnUiThread(() -> grantWebCamera(request));
            }

            @Override
            public void onPermissionRequestCanceled(PermissionRequest request) {
                pendingWebPermission = null;
            }

            @Override
            public boolean onShowFileChooser(WebView webView, ValueCallback<Uri[]> filePathCallback, FileChooserParams fileChooserParams) {
                if (fileCallback != null) fileCallback.onReceiveValue(null);
                fileCallback = filePathCallback;
                Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                intent.setType("image/*");
                intent.addCategory(Intent.CATEGORY_OPENABLE);
                try {
                    fileLauncher.launch(Intent.createChooser(intent, "Фото QR"));
                    return true;
                } catch (Exception e) {
                    fileCallback = null;
                    return false;
                }
            }
        });

        btnJoin.setOnClickListener(v -> openApp("join"));
        btnCreate.setOnClickListener(v -> openApp("register"));
        // Вкладка входа: там живёт и вход через Google. Раньше попасть на неё
        // со стартового экрана было нельзя.
        btnLogin.setOnClickListener(v -> openApp("login"));

        // Сразу показываем тот же экран, что и сайт. Настройка адреса
        // остаётся доступной: кнопка «назад» из интерфейса возвращает сюда.
        if (!savedRelay.isEmpty()) {
            loadWeb("login");
        }

        showSetupHint();
        askNotify();
    }

    private void grantWebCamera(PermissionRequest request) {
        boolean requestsCamera = Arrays.asList(request.getResources())
                .contains(PermissionRequest.RESOURCE_VIDEO_CAPTURE);
        if (!isTrustedLocalOrigin(request.getOrigin()) || !requestsCamera) {
            request.deny();
            return;
        }
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
            pendingWebPermission = request;
            ActivityCompat.requestPermissions(this, new String[]{Manifest.permission.CAMERA}, 44);
            return;
        }
        request.grant(new String[]{PermissionRequest.RESOURCE_VIDEO_CAPTURE});
    }

    public void startNativeScan() {
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
            pendingMode = "scan";
            ActivityCompat.requestPermissions(this, new String[]{Manifest.permission.CAMERA}, 45);
            return;
        }
        ScanOptions options = new ScanOptions();
        options.setDesiredBarcodeFormats(ScanOptions.QR_CODE);
        options.setPrompt("Наведите на QR-приглашение группы");
        options.setBeepEnabled(false);
        options.setOrientationLocked(false);
        options.setCameraId(0);
        options.setCaptureActivity(com.journeyapps.barcodescanner.CaptureActivity.class);
        qrLauncher.launch(options);
    }

    private class JsBridge {
        @android.webkit.JavascriptInterface
        public String lanOrigin() {
            // Узла на телефоне нет — интерфейс сам возьмёт свой origin.
            return "";
        }

        @android.webkit.JavascriptInterface
        public String localOrigin() {
            return serverOrigin;
        }

        @android.webkit.JavascriptInterface
        public void scanQr() {
            runOnUiThread(MainActivity.this::startNativeScan);
        }
    }

    /**
     * Подсказка на экране настройки. Узел на телефоне больше не поднимается:
     * данные живут на сервере, поэтому приложению нужен только его адрес.
     */
    private void showSetupHint() {
        String saved = getSharedPreferences(PREFS, MODE_PRIVATE).getString(KEY_RELAY, "");
        lanHint.setText(saved.isEmpty()
                ? "Укажите адрес сервера MeshKeeper, например https://meshkeeper.example.com"
                : "Сервер: " + saved);
    }

    private void askNotify() {
        if (Build.VERSION.SDK_INT >= 33) {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
                ActivityCompat.requestPermissions(this, new String[]{Manifest.permission.POST_NOTIFICATIONS}, 43);
            }
        }
    }

    private void openApp(String mode) {
        pendingMode = mode;
        loadWeb(mode);
    }

    private void loadWeb(String mode) {
        String relay;
        try {
            relay = normalizeRelay(serverUrl.getText().toString());
        } catch (IllegalArgumentException e) {
            Toast.makeText(this, e.getMessage(), Toast.LENGTH_LONG).show();
            return;
        }
        if (relay.isEmpty()) {
            Toast.makeText(this, "Укажите адрес сервера", Toast.LENGTH_LONG).show();
            return;
        }
        getSharedPreferences(PREFS, MODE_PRIVATE).edit().putString(KEY_RELAY, relay).apply();
        serverOrigin = relay;
        setup.setVisibility(View.GONE);
        web.setVisibility(View.VISIBLE);
        web.loadUrl(relay + "/login?app=1&mode=" + mode);
    }

    @Override
    protected void onPause() {
        super.onPause();
        // Без этого cookie живут только в памяти: система убивает процесс, и
        // сессия пропадает — приложение просит вход при каждом открытии,
        // хотя человек никуда не выходил.
        CookieManager.getInstance().flush();
    }

    @Override
    public void onBackPressed() {
        if (web.getVisibility() == View.VISIBLE && web.canGoBack()) {
            web.goBack();
            return;
        }
        if (web.getVisibility() == View.VISIBLE) {
            web.setVisibility(View.GONE);
            setup.setVisibility(View.VISIBLE);
            showSetupHint();
            return;
        }
        super.onBackPressed();
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, @NonNull String[] permissions, @NonNull int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        boolean granted = grantResults.length > 0 && grantResults[0] == PackageManager.PERMISSION_GRANTED;
        if (requestCode == 44 && pendingWebPermission != null) {
            if (granted && isTrustedLocalOrigin(pendingWebPermission.getOrigin())) {
                pendingWebPermission.grant(new String[]{PermissionRequest.RESOURCE_VIDEO_CAPTURE});
            }
            else pendingWebPermission.deny();
            pendingWebPermission = null;
            return;
        }
        if (requestCode == 45 && granted) startNativeScan();
    }

    /**
     * Доверяем ровно тому серверу, который настроен в приложении.
     *
     * Раньше здесь был localhost — приложение носило внутри собственную копию
     * бэкенда на Java. Эта копия должна была повторять каждое изменение
     * основного узла и неизбежно отставала, поэтому телефон стал тонким
     * клиентом сервера. Проверка осталась строгой: схема, хост и порт должны
     * совпасть, всё остальное уходит во внешний браузер.
     */
    /**
     * Страницы входа Google, которым разрешено открываться внутри приложения.
     *
     * Список узкий намеренно: это исключение из правила «чужое — во внешний
     * браузер», и расширять его на весь google.com нельзя — тогда любая
     * ссылка на сервисы Google открывалась бы внутри учётной программы.
     */
    private boolean isGoogleSignIn(Uri uri) {
        if (uri == null || !"https".equalsIgnoreCase(uri.getScheme())) return false;
        String host = uri.getHost();
        if (host == null) return false;
        host = host.toLowerCase(java.util.Locale.ROOT);
        return host.equals("accounts.google.com")
                || host.equals("accounts.youtube.com")
                || host.equals("myaccount.google.com")
                || host.endsWith(".gstatic.com")
                || host.equals("ssl.gstatic.com");
    }

    private boolean isTrustedLocalOrigin(Uri uri) {
        if (uri == null || serverOrigin.isEmpty()) return false;
        Uri trusted = Uri.parse(serverOrigin);
        return "https".equalsIgnoreCase(uri.getScheme())
                && uri.getHost() != null
                && uri.getHost().equalsIgnoreCase(trusted.getHost())
                && uri.getPort() == trusted.getPort();
    }

    private static String normalizeRelay(String raw) {
        String value = raw == null ? "" : raw.trim().replaceAll("/+$", "");
        if (value.isEmpty()) return "";
        if (!value.contains("://")) value = "https://" + value;
        Uri uri = Uri.parse(value);
        if (!"https".equalsIgnoreCase(uri.getScheme()) || uri.getHost() == null) {
            throw new IllegalArgumentException("Сервер синхронизации должен использовать HTTPS");
        }
        return uri.toString();
    }
}
