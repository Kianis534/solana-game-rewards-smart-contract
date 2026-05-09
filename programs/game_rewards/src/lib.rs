use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, MintTo, Transfer};

declare_id!("iGnCrZPsuWT6oNcha1bHvN9t5ChKWet9YNDGtXSn7oY");

#[program]
pub mod game_rewards {
    use super::*;

    /// Initializes the global game configuration.
    /// This sets up the mint authority, treasury, and emission parameters.
    pub fn initialize_game_config(
        ctx: Context<InitializeGameConfig>,
        min_players: u64,
        base_emission: u64,
        halving_interval: u64,
    ) -> Result<()> {
        let config = &mut ctx.accounts.game_config;
        let clock = Clock::get()?;

        config.admin = ctx.accounts.admin.key();
        config.token_mint = ctx.accounts.token_mint.key();
        config.reward_receiver = ctx.accounts.reward_receiver.key();
        config.vault_token_account = ctx.accounts.vault_token_account.key();
        
        config.min_players_condition = min_players;
        config.total_players = 0;
        
        config.base_daily_emission = base_emission;
        config.current_daily_emission = base_emission;
        config.halving_interval = halving_interval;
        
        config.launch_day = clock.unix_timestamp / 86400;
        config.last_emission_day = 0; // Not yet processed
        
        config.total_minted = 0;
        config.total_skipped_emission = 0;
        config.total_distributed = 0;
        
        config.paused = false;
        
        config.config_bump = ctx.bumps.game_config;
        config.mint_auth_bump = ctx.bumps.mint_authority;
        config.vault_auth_bump = ctx.bumps.vault_authority;

        emit!(GameConfigInitialized {
            admin: config.admin,
            mint: config.token_mint,
        });

        Ok(())
    }

    /// Processes the daily token emission.
    /// Can only be called once per day by the admin or authorized keeper.
    /// Calculates halving and checks if player count condition is met.
    pub fn process_daily_emission(ctx: Context<ProcessDailyEmission>) -> Result<()> {
        let config = &mut ctx.accounts.game_config;
        require!(!config.paused, GameError::ProgramPaused);

        let clock = Clock::get()?;
        let current_day = clock.unix_timestamp / 86400;

        // Ensure daily execution
        require!(
            config.last_emission_day < current_day,
            GameError::EmissionAlreadyProcessedToday
        );

        // Calculate current emission based on halving logic
        let days_since_launch = current_day.saturating_sub(config.launch_day) as u64;
        let halvings = days_since_launch.checked_div(config.halving_interval).unwrap_or(0);
        
        // Emission = base_emission / 2^halvings
        let mut current_emission = config.base_daily_emission;
        for _ in 0..halvings {
            current_emission = current_emission.checked_div(2).unwrap_or(0);
            if current_emission == 0 { break; }
        }
        config.current_daily_emission = current_emission;

        // Check player count condition
        if config.total_players >= config.min_players_condition {
            // Mint to reward receiver
            if current_emission > 0 {
                let seeds = &[
                    b"mint_authority".as_ref(),
                    &[config.mint_auth_bump],
                ];
                let signer = &[&seeds[..]];

                let cpi_accounts = MintTo {
                    mint: ctx.accounts.token_mint.to_account_info(),
                    to: ctx.accounts.reward_receiver.to_account_info(),
                    authority: ctx.accounts.mint_authority.to_account_info(),
                };
                let cpi_program = ctx.accounts.token_program.to_account_info();
                let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);

                token::mint_to(cpi_ctx, current_emission)?;

                config.total_minted = config.total_minted.checked_add(current_emission).ok_or(GameError::MathOverflow)?;
                config.total_distributed = config.total_distributed.checked_add(current_emission).ok_or(GameError::MathOverflow)?;
            }
        } else {
            // Player condition not met: Record as skipped/burned
            config.total_skipped_emission = config.total_skipped_emission.checked_add(current_emission).ok_or(GameError::MathOverflow)?;
        }

        config.last_emission_day = current_day;

        emit!(DailyEmissionProcessed {
            day: current_day,
            amount: current_emission,
            success: config.total_players >= config.min_players_condition,
        });

        Ok(())
    }

    /// Update total player count (Admin only)
    pub fn update_player_count(ctx: Context<AdminOnly>, new_count: u64) -> Result<()> {
        ctx.accounts.game_config.total_players = new_count;
        Ok(())
    }

    /// Update minimum required players (Admin only)
    pub fn update_min_players(ctx: Context<AdminOnly>, new_min: u64) -> Result<()> {
        ctx.accounts.game_config.min_players_condition = new_min;
        Ok(())
    }

    /// Update reward receiver address (Admin only)
    pub fn update_reward_receiver(ctx: Context<UpdateRewardReceiver>) -> Result<()> {
        ctx.accounts.game_config.reward_receiver = ctx.accounts.new_reward_receiver.key();
        Ok(())
    }

    /// Pause or unpause the program (Admin only)
    pub fn set_paused(ctx: Context<AdminOnly>, paused: bool) -> Result<()> {
        ctx.accounts.game_config.paused = paused;
        Ok(())
    }

    /// Transfer admin authority (Admin only)
    pub fn transfer_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
        ctx.accounts.game_config.admin = new_admin;
        Ok(())
    }

    /// Deposit tokens into the program vault
    pub fn deposit_tokens(ctx: Context<DepositTokens>, amount: u64) -> Result<()> {
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;
        Ok(())
    }

    /// Withdraw tokens from the program vault (Admin only)
    pub fn withdraw_tokens(ctx: Context<WithdrawTokens>, amount: u64) -> Result<()> {
        let config = &ctx.accounts.game_config;
        let seeds = &[
            b"vault_authority".as_ref(),
            &[config.vault_auth_bump],
        ];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault_token_account.to_account_info(),
            to: ctx.accounts.destination_token_account.to_account_info(),
            authority: ctx.accounts.vault_authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);
        token::transfer(cpi_ctx, amount)?;
        Ok(())
    }
}

#[derive(Accounts)]
pub struct InitializeGameConfig<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + GameConfig::SIZE,
        seeds = [b"game_config"],
        bump
    )]
    pub game_config: Account<'info, GameConfig>,

    /// PDA that will have Mint Authority over the reward token
    /// CHECK: PDA used only as authority
    #[account(
        seeds = [b"mint_authority"],
        bump
    )]
    pub mint_authority: UncheckedAccount<'info>,

    /// PDA that will have Authority over the vault token account
    /// CHECK: PDA used only as authority
    #[account(
        seeds = [b"vault_authority"],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    pub token_mint: Account<'info, Mint>,

    /// The initial reward receiver (ATA)
    pub reward_receiver: Account<'info, TokenAccount>,

    /// The program's vault token account
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ProcessDailyEmission<'info> {
    #[account(
        mut,
        seeds = [b"game_config"],
        bump = game_config.config_bump,
        has_one = admin,
    )]
    pub game_config: Account<'info, GameConfig>,

    #[account(
        mut,
        address = game_config.token_mint
    )]
    pub token_mint: Account<'info, Mint>,

    /// CHECK: PDA authority
    #[account(
        seeds = [b"mint_authority"],
        bump = game_config.mint_auth_bump
    )]
    pub mint_authority: UncheckedAccount<'info>,

    #[account(
        mut,
        address = game_config.reward_receiver
    )]
    pub reward_receiver: Account<'info, TokenAccount>,

    pub admin: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    #[account(
        mut,
        seeds = [b"game_config"],
        bump = game_config.config_bump,
        has_one = admin
    )]
    pub game_config: Account<'info, GameConfig>,

    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct UpdateRewardReceiver<'info> {
    #[account(
        mut,
        seeds = [b"game_config"],
        bump = game_config.config_bump,
        has_one = admin
    )]
    pub game_config: Account<'info, GameConfig>,

    pub new_reward_receiver: Account<'info, TokenAccount>,

    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct DepositTokens<'info> {
    #[account(
        seeds = [b"game_config"],
        bump = game_config.config_bump,
    )]
    pub game_config: Account<'info, GameConfig>,

    #[account(
        mut,
        address = game_config.vault_token_account
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,

    pub user: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct WithdrawTokens<'info> {
    #[account(
        seeds = [b"game_config"],
        bump = game_config.config_bump,
        has_one = admin
    )]
    pub game_config: Account<'info, GameConfig>,

    /// CHECK: PDA authority
    #[account(
        seeds = [b"vault_authority"],
        bump = game_config.vault_auth_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(
        mut,
        address = game_config.vault_token_account
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub destination_token_account: Account<'info, TokenAccount>,

    pub admin: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[account]
pub struct GameConfig {
    pub admin: Pubkey,
    pub token_mint: Pubkey,
    pub reward_receiver: Pubkey,
    pub vault_token_account: Pubkey,
    
    pub min_players_condition: u64,
    pub total_players: u64,
    
    pub base_daily_emission: u64,
    pub current_daily_emission: u64,
    pub halving_interval: u64, // in days
    
    pub launch_day: i64,
    pub last_emission_day: i64,
    
    pub total_minted: u64,
    pub total_skipped_emission: u64,
    pub total_distributed: u64,
    
    pub paused: bool,
    
    pub config_bump: u8,
    pub mint_auth_bump: u8,
    pub vault_auth_bump: u8,
}

impl GameConfig {
    pub const SIZE: usize = 32 + 32 + 32 + 32 // Pubkeys
        + 8 + 8 // Conditions/Players
        + 8 + 8 + 8 // Emission/Halving
        + 8 + 8 // Days
        + 8 + 8 + 8 // Totals
        + 1 // Paused
        + 1 + 1 + 1; // Bumps
}

#[event]
pub struct GameConfigInitialized {
    pub admin: Pubkey,
    pub mint: Pubkey,
}

#[event]
pub struct DailyEmissionProcessed {
    pub day: i64,
    pub amount: u64,
    pub success: bool,
}

#[error_code]
pub enum GameError {
    #[msg("The program is currently paused.")]
    ProgramPaused,
    #[msg("Emission has already been processed for today.")]
    EmissionAlreadyProcessedToday,
    #[msg("Arithmetic overflow occurred.")]
    MathOverflow,
}
