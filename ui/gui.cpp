#include <boost/log/trivial.hpp>
#include "ui.hpp"
#include "gui.hpp"

#include <string>

#ifdef _WIN64
#include "SDL.h"
#include "SDL_ttf.h"
#else
#include "SDL2/SDL.h"
#include "SDL2/SDL_ttf.h"
#endif

#define SCREEN_WIDTH 1024
#define SCREEN_HEIGHT 768

void ModuleWheel::module_wheel_moved() { };

void ModuleWheel::render_text(std::string text, SDL_Surface *target_surface, int height) {
    SDL_Surface *text_surface;
    if(!(text_surface=TTF_RenderUTF8_Blended(this->font,text.c_str(),this->fg_color))) {
        std::cout << "font rendering failed\n";
    } else {
        SDL_Rect dstrect;
        dstrect.x = 20;
        dstrect.y = height;
        SDL_BlitSurface(text_surface,NULL,target_surface,&dstrect);
        //perhaps we can reuse it, but I assume not for simplicity.
        SDL_FreeSurface(text_surface);
    }
}

void ModuleWheel::render_modules()
{
    int num_modules = 0;
    int vertical_center = this->surface->h / 2;
    int height = 0;

    //fixme: maybe forward_list was not the most forward_thinking data structure
    for (auto module: this->module_list) num_modules++;

    for (auto module: this->module_list) {
        height = vertical_center;
        render_text(module.label, this->surface, height);
    }
}

SDL_Surface *ModuleWheel::render() {
    SDL_FillRect(this->surface, 0, SDL_MapRGB(this->surface->format, this->bg_color.r, this->bg_color.b, this->bg_color.g ));
    this->render_modules();
    return this->surface;
}

ModuleWheel::ModuleWheel(ui_module_list_t module_list) {
    this->module_list = module_list;
    this->surface = SDL_CreateRGBSurface(0, SCREEN_WIDTH, SCREEN_HEIGHT, 32, 0,0,0,0);

    this->font=TTF_OpenFont("fonts/FreeSansBold.ttf", 28);
    if(!this->font) {
        printf("TTF_OpenFont: %s\n", TTF_GetError());
        // handle error
    }
	this->font_line_height = TTF_FontLineSkip(this->font);
    SDL_FillRect(this->surface, 0, SDL_MapRGB(this->surface->format, 255, 0, 0));
}

bool BS5gui::sdl_init()
{
    //Initialization flag
    bool success = true;

    //initialize TTF
    if(TTF_Init()==-1) {
        printf("TTF_Init: %s\n", TTF_GetError());
        exit(2);
    }

    //Initialize SDL
    if( SDL_Init( SDL_INIT_VIDEO ) < 0 )
    {
        printf( "SDL could not initialize! SDL_Error: %s\n", SDL_GetError() );
        success = false;
    }
    else
    {
        //Create window
        this->window = SDL_CreateWindow( "neomedia", SDL_WINDOWPOS_UNDEFINED, SDL_WINDOWPOS_UNDEFINED, SCREEN_WIDTH, SCREEN_HEIGHT, SDL_WINDOW_SHOWN);
        if( this->window == NULL )
        {
            printf( "Window could not be created! SDL_Error: %s\n", SDL_GetError() );
            success = false;
        }
        else
        {
            //Get window surface
            this->window_surface = SDL_GetWindowSurface( this->window );
        }
    }

    return success;
}

void BS5gui::input_callback(BS5input::event_t event, BS5input::bs5_state_t state) {
    BOOST_LOG_TRIVIAL(debug) << "Callback fired";
}

BS5gui::BS5gui(ui_module_list_t & ui_module_list) {
    sdl_init();
    this->module_wheel = new ModuleWheel(ui_module_list);
    this->layer_list.push_front(&this->module_wheel->surface);
    render();
}

void BS5gui::render() {
    BOOST_LOG_TRIVIAL(debug) << "Rendering frame";
	if (this->panel != nullptr) {
		this->module_wheel->bg_color = {
			(uint8_t)(this->panel->input.bs5_state.posWheel1 % 255),
			(uint8_t)(this->panel->input.bs5_state.posWheel2 % 255),
			(uint8_t)(this->panel->input.bs5_state.posWheel3),
					(uint8_t)255 };
	} else {
		this->module_wheel->bg_color = {0xff, 0xf0, 0x0f, 0xff};
	}
    this->module_wheel->render();
    for(auto layer: this->layer_list) {
        SDL_BlitSurface(*layer, 0, this->window_surface, 0);
    }
    SDL_UpdateWindowSurface(this->window);
}
