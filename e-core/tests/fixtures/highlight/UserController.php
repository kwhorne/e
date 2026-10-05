<?php

namespace App\Http\Controllers;

use App\Models\User;
use Illuminate\Http\Request;
use Illuminate\Support\Facades\DB;

/**
 * Users, with a little raw SQL for the reports.
 */
final class UserController extends Controller
{
    public const PER_PAGE = 25;

    protected array $casts = ['email_verified_at' => 'datetime'];

    public function index(Request $request): \Illuminate\View\View
    {
        $users = User::query()
            ->where('active', true)
            ->whereRaw('created_at > NOW() - INTERVAL 30 DAY')
            ->orderByRaw("FIELD(role, 'admin', 'editor') DESC")
            ->paginate(self::PER_PAGE);

        // Totals per role.
        $totals = DB::select("SELECT role, COUNT(*) AS n FROM users WHERE team_id = ? GROUP BY role", [$request->user()->team_id]);

        $title = "Users for {$request->user()->name} ({$users->total()})";
        $note = <<<EOT
            Generated at {$request->date}
            EOT;

        return view('users.index', compact('users', 'totals', 'title', 'note'));
    }

    public function store(Request $request)
    {
        $data = $request->validate([
            'name' => ['required', 'string', 'max:255'],
            'email' => 'required|email|unique:users,email',
        ]);
        # legacy comment style
        $user = User::create($data + ['password' => bcrypt(str()->random(32))]);

        DB::statement('UPDATE users SET last_seen = NULL WHERE id = ' . (int) $user->id);

        return match (true) {
            $user->isAdmin() => redirect()->route('admin.users.show', $user),
            default => to_route('users.show', ['user' => $user->id]),
        };
    }

    private function score(int|float $x, ?string $label = null): float
    {
        $fn = fn ($v) => $v * 2.5e3 + 0x1F;
        return $fn($x) / max(1, strlen($label ?? ''));
    }
}
