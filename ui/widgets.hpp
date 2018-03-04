#pragma once
#ifndef WIDGETS_HPP

#define WIDGETS_HPP

#ifdef _WIN64
#include "SDL.h"
#include "SDL_ttf.h"
#else
#include "SDL2/SDL.h"
#include "SDL2/SDL_ttf.h"
#endif

#include <list>

#include "ui/gui.hpp"

class NeomasterUI;

class NMWidget {
public:
	virtual	 void draw() = 0;
protected:
	NeomasterUI * ui = nullptr;
};

class NMProgressBar : public NMWidget {
public:
	NMProgressBar(NeomasterUI * ui, int x, int y, int h, int w);
	void set_value(float value);
	void draw();
private:
	SDL_Rect frame, bar, gap;
	int x, y, h, w;
};

class NMImage : public NMWidget {

};
#endif