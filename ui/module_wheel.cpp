#include <boost/log/trivial.hpp>
#include "ui.hpp"
#include "gui.hpp"
#include <map>
#include <string>
#include <cmath>
#include "ui/module_wheel.hpp"

#ifdef _WIN64
#include "SDL.h"
#include "SDL_ttf.h"
#include "SDL_image.h"
#else
#include "SDL2/SDL.h"
#include "SDL2/SDL_ttf.h"
#include "SDL2/SDL_image.h"
#endif

typedef std::pair<SDL_Texture *, SDL_Rect> text_texture_t;
std::map<std::string, text_texture_t> text_texture_map; // this could be specialised to automatically free surfaces
SDL_Texture * background_texture = nullptr;
void ModuleWheel::module_wheel_moved() { };

void ModuleWheel::render_background() {
	if (background_texture == nullptr) {
		std::string background_image_path = "bitmaps/module_wheel_1.png";
		SDL_Surface* loadedSurface = IMG_Load(background_image_path.c_str());
		if (loadedSurface == NULL)
		{
			printf("Unable to load image %s! SDL_image Error: %s\n", background_image_path.c_str(), IMG_GetError());
		}
		else
		{
			//Convert surface to screen format
			background_texture = SDL_CreateTextureFromSurface(this->renderer, loadedSurface);
			if (background_texture == NULL)
			{
				printf("Unable to optimize image %s! SDL Error: %s\n", background_image_path.c_str(), SDL_GetError());
			}

			//Get rid of old loaded surface
			SDL_FreeSurface(loadedSurface);
		}
	}
	SDL_RenderCopy(this->renderer, background_texture, NULL, NULL);
}

//FIXME: This function and its associated data types will need to not only deal with
// one single surface, but a pair of textures, with and without hilight.
void ModuleWheel::render_text(const std::string & text, SDL_Rect rect) {
	SDL_Texture *text_texture;
	SDL_Surface *text_surface;
	// See if the text surface has already been cached
	auto text_iterator = text_texture_map.find(text);
	if (text_iterator != text_texture_map.end()) {
		text_texture = text_iterator->second.first;
		rect.w = text_iterator->second.second.w;
		rect.h = text_iterator->second.second.h;
	} else {
		if (!(text_surface = TTF_RenderUTF8_Blended(this->font, text.c_str(), this->fg_color))) {
			std::cout << "font rendering failed\n";
		}
		else {
			rect.w = text_surface->w;
			rect.h = text_surface->h;
			text_texture = SDL_CreateTextureFromSurface(this->renderer, text_surface);
			text_texture_map[text] = text_texture_t(text_texture, rect);
		}
		SDL_FreeSurface(text_surface);
	}
	SDL_RenderCopy(this->renderer, text_texture, NULL, &rect);
}

SDL_Rect ModuleWheel::get_rekt(int num_modules, int module_index) {
	int vertical_center = (SCREEN_HEIGHT / 2) - (this->font_line_height / 2);
	int vertical_spacing = this->font_line_height * 2;
	SDL_Rect rekt = { 0,0,0,0 };

	// If there are so few modules that we can fit them all by going upwards, 
	// we do so; otherwise we grow downwards.
	if ((vertical_center - (vertical_spacing * num_modules)) <= 0) {
		BOOST_LOG_TRIVIAL(error) << "Too many modules, the distributor can't handle that yet";
	}
	else {
		rekt.y = (vertical_center - (vertical_spacing * module_index));
		rekt.x = 40 + (abs(vertical_spacing * module_index) / 8);
	}
	return rekt;
}

void ModuleWheel::render_beam() {
	int x1, y1, x2, y2;
	y1 = (this->input_state.posWheel3 / 127.0) * (SCREEN_HEIGHT + 200) - 100;
	x1 = 0;
	y2 = SCREEN_HEIGHT / 2;
	x2 = SCREEN_WIDTH;
	SDL_SetRenderDrawColor(this->renderer, 255, 255, 255, 50);
	SDL_RenderDrawLine(this->renderer, x1, y1, x2, y2);
}

void ModuleWheel::handle_panel_input(const BS5input::bs5_damage_t & damage, const BS5input::bs5_state_t & state) {
	if (damage.wheel_3)
	{
		this->input_state = state;
	}
}

void ModuleWheel::render_modules()
{
	int num_modules = 0;
	int module_index = 0;

	int height = 0;

	//fixme: maybe forward_list was not the most forward_thinking data structure
	for (auto module : this->module_list) num_modules++;

	for (auto module : this->module_list) {
		render_text(module->label, get_rekt(num_modules, module_index++));
	}
}

void ModuleWheel::render() {
	//SDL_SetRenderDrawColor(this->renderer, this->bg_color.r, this->bg_color.b, this->bg_color.g, 255);
	//SDL_RenderClear(this->renderer);
	render_background();
	render_modules();
	render_beam();
}

ModuleWheel::ModuleWheel(SDL_Renderer * renderer, ui_module_list_t module_list) {
	this->module_list = module_list;
	this->font = TTF_OpenFont("fonts/FreeSansBold.ttf", 28);
	if (!this->font) {
		printf("TTF_OpenFont: %s\n", TTF_GetError());
		// handle error
	}
	this->renderer = renderer;
	this->font_line_height = TTF_FontLineSkip(this->font);

	//Initialize PNG loading
	int imgFlags = IMG_INIT_PNG;
	if (!(IMG_Init(imgFlags) & imgFlags))
	{
		BOOST_LOG_TRIVIAL(error) << "SDL_image could not initialize! SDL_image Error: " << IMG_GetError();
	}
}