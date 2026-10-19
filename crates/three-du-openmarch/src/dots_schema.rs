//! The database layout OpenMarch desktop expects in a `.dots` file.
//!
//! This is an independent statement of the schema that OpenMarch's Drizzle migrations
//! 0000–0016 produce (OpenMarch 0.1.7 and `main` as of 2026-10). It is written here for
//! interoperability; no OpenMarch source is bundled. `tests/` compares it with the real
//! migration output when a local OpenMarch checkout is available.

/// `PRAGMA user_version` OpenMarch requires before it will apply Drizzle migrations.
pub const USER_VERSION: i32 = 7;

/// Rows of `__drizzle_migrations`: (sha256 of the migration file, folder timestamp).
/// Drizzle applies any migration newer than the last `created_at`, so recording all of
/// them tells OpenMarch the file is current. Hashes are of the LF-normalized files.
pub const MIGRATIONS: &[(&str, i64)] = &[
    ("8ab8dbe3589a3675be4330f30277d0e0e3ace1a2ea26c53becc4aeeb95822160", 1749812963333),
    ("3f61f2928dc743a2319d8122acba5a7f33d0d43dbbaaeb8fc61a0f12c9587e29", 1752789274792),
    ("334299e4161d49e5f88a59c6959669356aed78016c93f30265105dfd1d8e00f1", 1752789863048),
    ("033e932835eea513074c5777fbe594cfb444ca980505ab110bbce3607b5229ff", 1753068380361),
    ("5e1c0b53722d57364da083b938ba86aae7ddfc9c4b6bb74f12bfea1690295cb0", 1753640028768),
    ("5d110f1181826c600a0d0fb2d6b599670df600dc174fc0625898ce5f046e79d3", 1754832111507),
    ("6f5422ad292839b76eae9873ff8135f298cb60d3fa9c32e8e970ef96c55fa33b", 1755743220373),
    ("8139ac2f7772fa7d575d1acd0f0ae5c53cccf94259a465feaf3256da74c25449", 1757474673889),
    ("a5735fe11cadca314c77677cb3574e1a2ade486a9732252f68c4e90d0f642f98", 1758910347493),
    ("8836c61bb084431c3f5eb2ad6d64d7b1783b75cafeb89589a67fd7954b2602b4", 1759086544223),
    ("b2ed737cb9eed39526f7f7540ed43ead542b1536fa660ed6c282827a1ead19ff", 1759286004269),
    ("7b4ed16cf545bef8c6a7ba8ccba2d18fa0103cd01bb53e0bdec7f7b054340a73", 1760733039979),
    ("4c49ffa8289bc3e1ef632a72fc7f1902ac59cd2dbceb00b74d28384527c2f243", 1764521848101),
    ("29d8101e03441833266669f00df651e9aa77e1f8c727054a2ce9f694ee5cab4a", 1764547615201),
    ("1809a63f412b2a0cd19bb36229b449f254e44506dcc46ad30a89207b88e9aedb", 1764557592093),
    ("6c16fc7b51c3f803f73057c018169f6db22151bc9b07b1a918d856a009ed9d00", 1764723930076),
    ("e40132ccaf70a6715877e42c371d8abe04fe57e7b5cdc69c81b182d0d428f714", 1764733345489),
];

const TS: &str = "`created_at` text DEFAULT (CURRENT_TIMESTAMP) NOT NULL, `updated_at` text DEFAULT (CURRENT_TIMESTAMP) NOT NULL";

pub fn ddl() -> String {
    format!(
        r#"
CREATE TABLE "__drizzle_migrations" (id SERIAL PRIMARY KEY, hash text NOT NULL, created_at numeric);
CREATE TABLE "audio_files" (`id` integer PRIMARY KEY NOT NULL, `path` text NOT NULL, `nickname` text, `data` blob,
    `selected` integer DEFAULT 0 NOT NULL, {TS});
CREATE TABLE "beats" (`id` integer PRIMARY KEY NOT NULL, `duration` real NOT NULL, `position` integer NOT NULL,
    `include_in_measure` integer DEFAULT 1 NOT NULL, `notes` text, {TS},
    CONSTRAINT "beats_duration_check" CHECK(duration >= 0),
    CONSTRAINT "beats_position_check" CHECK(position >= 0),
    CONSTRAINT "beats_include_in_measure" CHECK(include_in_measure IN (0, 1)));
CREATE TABLE "field_properties" (`id` integer PRIMARY KEY NOT NULL, `json_data` text NOT NULL, `image` blob,
    CONSTRAINT "field_properties_id_check" CHECK(id = 1));
CREATE TABLE "history_redo" (`sequence` integer PRIMARY KEY NOT NULL, `history_group` integer NOT NULL, `sql` text NOT NULL);
CREATE TABLE "history_stats" (`id` integer PRIMARY KEY NOT NULL, `cur_undo_group` integer NOT NULL, `cur_redo_group` integer NOT NULL,
    `group_limit` integer NOT NULL, CONSTRAINT "history_stats_id_check" CHECK(id = 1));
CREATE TABLE "history_undo" (`sequence` integer PRIMARY KEY NOT NULL, `history_group` integer NOT NULL, `sql` text NOT NULL);
CREATE TABLE "marchers" (`id` integer PRIMARY KEY NOT NULL, `name` text, `section` text NOT NULL, `year` text, `notes` text,
    `drill_prefix` text NOT NULL, `drill_order` integer NOT NULL, {TS});
CREATE TABLE "pages" (`id` integer PRIMARY KEY NOT NULL, `is_subset` integer DEFAULT 0 NOT NULL, `notes` text, {TS},
    `start_beat` integer NOT NULL,
    FOREIGN KEY (`start_beat`) REFERENCES `beats`(`id`) ON UPDATE no action ON DELETE no action,
    CONSTRAINT "pages_is_subset_check" CHECK(is_subset IN (0, 1)));
CREATE TABLE `pathways` (`id` integer PRIMARY KEY NOT NULL, "path_data" text NOT NULL, `notes` text, {TS});
CREATE TABLE "marcher_pages" (`id` integer PRIMARY KEY NOT NULL, `marcher_id` integer NOT NULL, `page_id` integer NOT NULL,
    `x` real NOT NULL, `y` real NOT NULL, {TS}, `path_data_id` integer, `path_start_position` real, `path_end_position` real,
    `notes` text, `rotation_degrees` real DEFAULT 0 NOT NULL, `fill_color` text, `outline_color` text, `shape_type` text,
    `visible` integer DEFAULT 1 NOT NULL, `label_visible` integer DEFAULT 1 NOT NULL, `equipment_name` text, `equipment_state` text,
    FOREIGN KEY (`marcher_id`) REFERENCES `marchers`(`id`) ON UPDATE no action ON DELETE cascade,
    FOREIGN KEY (`page_id`) REFERENCES `pages`(`id`) ON UPDATE no action ON DELETE cascade,
    FOREIGN KEY (`path_data_id`) REFERENCES `pathways`(`id`) ON UPDATE no action ON DELETE set null,
    CONSTRAINT "marcher_pages_path_data_position_check" CHECK(path_start_position >= 0 AND path_start_position <= 1 AND path_end_position >= 0 AND path_end_position <= 1));
CREATE TABLE `tags` (`id` integer PRIMARY KEY NOT NULL, `name` text, `description` text, {TS}, `icon` text, `color_hex` text);
CREATE TABLE `marcher_tags` (`id` integer PRIMARY KEY NOT NULL, `marcher_id` integer NOT NULL, `tag_id` integer NOT NULL, {TS},
    FOREIGN KEY (`marcher_id`) REFERENCES `marchers`(`id`) ON UPDATE no action ON DELETE cascade,
    FOREIGN KEY (`tag_id`) REFERENCES `tags`(`id`) ON UPDATE no action ON DELETE cascade);
CREATE TABLE "measures" (`id` integer PRIMARY KEY NOT NULL, `start_beat` integer NOT NULL, `rehearsal_mark` text, `notes` text, {TS},
    FOREIGN KEY (`start_beat`) REFERENCES `beats`(`id`) ON UPDATE no action ON DELETE no action);
CREATE TABLE "midsets" (`id` integer PRIMARY KEY NOT NULL, `mp_id` integer NOT NULL, `x` real NOT NULL, `y` real NOT NULL,
    `progress_placement` real NOT NULL, {TS}, `path_data_id` integer, `path_start_position` real, `path_end_position` real, `notes` text,
    FOREIGN KEY (`mp_id`) REFERENCES `marcher_pages`(`id`) ON UPDATE no action ON DELETE cascade,
    FOREIGN KEY (`path_data_id`) REFERENCES `pathways`(`id`) ON UPDATE no action ON DELETE set null,
    CONSTRAINT "midsets_path_data_position_check" CHECK(path_start_position >= 0 AND path_start_position <= 1 AND path_end_position >= 0 AND path_end_position <= 1),
    CONSTRAINT "placement_check" CHECK(progress_placement > 0 AND progress_placement < 1));
CREATE TABLE "section_appearances" (`id` integer PRIMARY KEY NOT NULL, `section` text NOT NULL, `fill_color` text, `outline_color` text,
    `shape_type` text, `visible` integer DEFAULT 1 NOT NULL, `label_visible` integer DEFAULT 1 NOT NULL, `equipment_name` text,
    `equipment_state` text, {TS});
CREATE TABLE "shapes" (`id` integer PRIMARY KEY NOT NULL, `name` text, {TS}, `notes` text);
CREATE TABLE "shape_pages" (`id` integer PRIMARY KEY NOT NULL, `shape_id` integer NOT NULL, `page_id` integer NOT NULL,
    `svg_path` text NOT NULL, {TS}, `notes` text,
    FOREIGN KEY (`shape_id`) REFERENCES `shapes`(`id`) ON UPDATE no action ON DELETE cascade,
    FOREIGN KEY (`page_id`) REFERENCES `pages`(`id`) ON UPDATE no action ON DELETE cascade);
CREATE TABLE "shape_page_marchers" (`id` integer PRIMARY KEY NOT NULL, `shape_page_id` integer NOT NULL, `marcher_id` integer NOT NULL,
    `position_order` integer NOT NULL, {TS}, `notes` text,
    FOREIGN KEY (`shape_page_id`) REFERENCES `shape_pages`(`id`) ON UPDATE no action ON DELETE cascade,
    FOREIGN KEY (`marcher_id`) REFERENCES `marchers`(`id`) ON UPDATE no action ON DELETE cascade);
CREATE TABLE `tag_appearances` (`id` integer PRIMARY KEY NOT NULL, `tag_id` integer NOT NULL, `start_page_id` integer NOT NULL,
    `fill_color` text, `outline_color` text, `shape_type` text, `visible` integer DEFAULT 1 NOT NULL, `label_visible` integer DEFAULT 1 NOT NULL,
    `priority` integer DEFAULT 0 NOT NULL, {TS}, `equipment_name` text, `equipment_state` text,
    FOREIGN KEY (`tag_id`) REFERENCES `tags`(`id`) ON UPDATE no action ON DELETE cascade,
    FOREIGN KEY (`start_page_id`) REFERENCES `pages`(`id`) ON UPDATE no action ON DELETE cascade);
CREATE TABLE "utility" (`id` integer PRIMARY KEY NOT NULL, `last_page_counts` integer DEFAULT 8 NOT NULL,
    `default_beat_duration` real DEFAULT 0.5 NOT NULL, `updated_at` text DEFAULT (CURRENT_TIMESTAMP) NOT NULL,
    CONSTRAINT "utility_last_page_counts_check" CHECK(last_page_counts > 0),
    CONSTRAINT "utility_id_check" CHECK(id = 0),
    CONSTRAINT "utility_default_beat_duration_check" CHECK(default_beat_duration > 0));
CREATE TABLE `workspace_settings` (`id` integer PRIMARY KEY NOT NULL, `json_data` text NOT NULL, {TS},
    CONSTRAINT "workspace_settings_id_check" CHECK(id = 1));
CREATE INDEX `idx-spm-marcher_id` ON `shape_page_marchers` (`marcher_id`);
CREATE INDEX `idx-spm-shape_page_id` ON `shape_page_marchers` (`shape_page_id`);
CREATE INDEX `index_marcher_pages_on_marcher_id` ON `marcher_pages` (`marcher_id`);
CREATE INDEX `index_marcher_pages_on_page_id` ON `marcher_pages` (`page_id`);
CREATE UNIQUE INDEX `marcher_pages_marcher_id_page_id_unique` ON `marcher_pages` (`marcher_id`,`page_id`);
CREATE UNIQUE INDEX `marcher_tags_marcher_id_tag_id_unique` ON `marcher_tags` (`marcher_id`,`tag_id`);
CREATE UNIQUE INDEX `marchers_drill_prefix_drill_order_unique` ON `marchers` (`drill_prefix`,`drill_order`);
CREATE UNIQUE INDEX `midsets_mp_id_progress_placement_unique` ON `midsets` (`mp_id`,`progress_placement`);
CREATE UNIQUE INDEX `pages_start_beat_unique` ON `pages` (`start_beat`);
CREATE UNIQUE INDEX `shape_page_marchers_shape_page_id_marcher_id_unique` ON `shape_page_marchers` (`shape_page_id`,`marcher_id`);
CREATE UNIQUE INDEX `shape_page_marchers_shape_page_id_position_order_unique` ON `shape_page_marchers` (`shape_page_id`,`position_order`);
CREATE UNIQUE INDEX `shape_pages_page_id_shape_id_unique` ON `shape_pages` (`page_id`,`shape_id`);
CREATE UNIQUE INDEX `tag_appearances_tag_id_start_page_id_unique` ON `tag_appearances` (`tag_id`,`start_page_id`);
CREATE VIEW `timing_objects` AS
SELECT beats.position AS position, beats.duration AS duration,
    SUM(duration) OVER (ORDER BY position ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) AS timestamp,
    beats.id AS beat_id, pages.id AS page_id, measures.id AS measure_id
FROM beats
    LEFT JOIN pages ON beats.id = pages.start_beat
    LEFT JOIN measures ON beats.id = measures.start_beat;
"#
    )
}
