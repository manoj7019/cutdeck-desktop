using System; using System.Drawing; using System.Drawing.Drawing2D; using System.Windows.Forms; using System.Windows.Media; using System.Collections.Generic; using System.IO; using System.Runtime.InteropServices;
using Col = System.Drawing.Color;
class W : Form {
 [DllImport("gdi32.dll")] static extern IntPtr CreateRoundRectRgn(int a,int b,int c,int d,int e,int f);
 [DllImport("user32.dll")] static extern bool ReleaseCapture(); [DllImport("user32.dll")] static extern int SendMessage(IntPtr h,int m,int w,int l);
 MediaPlayer mp=new MediaPlayer(); List<string> files=new List<string>(); int cur=-1, loop=0; bool playing=false, open=false;
 string store=Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),"BobPlayer");
 Label title=new Label(), sub=new Label(), play, loopB, pin; Panel bar=new Panel(); ListBox list=new ListBox(); Timer t=new Timer();
 Col bg=Col.FromArgb(22,22,28), acc=Col.FromArgb(30,215,96), dim=Col.FromArgb(150,150,160);
 const int H=92, LH=210, WD=520;
 Label Ico(string g,int x,int y,int sz,EventHandler h){var l=new Label();l.Text=g;l.Font=new Font("Segoe MDL2 Assets",sz);l.ForeColor=Col.White;l.SetBounds(x,y,sz*2+8,sz*2+8);l.TextAlign=ContentAlignment.MiddleCenter;l.Cursor=Cursors.Hand;l.Click+=h;
  l.MouseEnter+=delegate{if(l.ForeColor==Col.White)l.ForeColor=acc;};l.MouseLeave+=delegate{if(l.ForeColor==acc&&l!=loopB&&l!=pin)l.ForeColor=Col.White;else if(l==loopB)Paint2();};Controls.Add(l);return l;}
 W(){
  Text="Bob Player"; FormBorderStyle=FormBorderStyle.None; BackColor=bg; ForeColor=Col.White; StartPosition=FormStartPosition.Manual; ShowInTaskbar=true; DoubleBuffered=true;
  var wa=Screen.PrimaryScreen.WorkingArea; Location=new Point(wa.Right-WD-20,wa.Bottom-H-20); Size(H);
  title.SetBounds(16,12,300,22); title.Font=new Font("Segoe UI Semibold",11); title.AutoEllipsis=true; title.Text="Bob Player";
  sub.SetBounds(16,34,300,18); sub.Font=new Font("Segoe UI",8.5f); sub.ForeColor=dim; sub.Text="Drop music here or click open";
  Controls.Add(title); Controls.Add(sub);
  Ico("\uE892",318,14,13,delegate{Go(cur-1);}); play=Ico("\uE768",352,8,18,delegate{Toggle();}); Ico("\uE893",398,14,13,delegate{Go(cur+1);});
  loopB=Ico("\uE8EE",436,18,10,delegate{loop=(loop+1)%3;Paint2();Save();});
  Ico("\uE8E5",462,18,10,delegate{Add();}); Ico("\uE8FD",488,18,10,delegate{open=!open;Size(open?H+LH:H);});
  Ico("\uE8BB",WD-24,2,7,delegate{Close();}); pin=Ico("\uE718",WD-46,2,7,delegate{TopMost=!TopMost;pin.ForeColor=TopMost?acc:Col.White;});
  bar.SetBounds(16,64,WD-32,6); bar.Cursor=Cursors.Hand; Controls.Add(bar);
  bar.Paint+=(o,e)=>{var g=e.Graphics;g.SmoothingMode=SmoothingMode.AntiAlias;g.Clear(bg);Fill(g,Col.FromArgb(55,55,65),0,bar.Width);double f=Frac();Fill(g,acc,0,(int)(bar.Width*f));};
  bar.MouseDown+=(o,e)=>{if(mp.NaturalDuration.HasTimeSpan)mp.Position=TimeSpan.FromSeconds(mp.NaturalDuration.TimeSpan.TotalSeconds*e.X/bar.Width);};
  list.SetBounds(10,H,WD-20,LH-10); list.BackColor=Col.FromArgb(30,30,38); list.ForeColor=Col.Gainsboro; list.BorderStyle=BorderStyle.None; list.Font=new Font("Segoe UI",9.5f); list.ItemHeight=24; list.DrawMode=DrawMode.OwnerDrawFixed;
  list.DrawItem+=(o,e)=>{if(e.Index<0)return;bool c=e.Index==cur;e.Graphics.FillRectangle(new SolidBrush((e.State&DrawItemState.Selected)!=0?Col.FromArgb(45,45,58):list.BackColor),e.Bounds);
   TextRenderer.DrawText(e.Graphics,(c?"\u25B6  ":"     ")+Path.GetFileNameWithoutExtension(files[e.Index]),list.Font,new Rectangle(e.Bounds.X+6,e.Bounds.Y,e.Bounds.Width-12,e.Bounds.Height),c?acc:Col.Gainsboro,TextFormatFlags.VerticalCenter|TextFormatFlags.EndEllipsis);};
  list.DoubleClick+=delegate{if(list.SelectedIndex>=0)Go(list.SelectedIndex);};
  list.KeyDown+=(o,e)=>{if(e.KeyCode==Keys.Delete&&list.SelectedIndex>=0){int i=list.SelectedIndex;files.RemoveAt(i);list.Items.RemoveAt(i);if(i<cur)cur--;else if(i==cur){mp.Stop();cur=-1;playing=false;play.Text="\uE768";}Save();}};
  Controls.Add(list);
  MouseDown+=Drag; title.MouseDown+=Drag; sub.MouseDown+=Drag;
  MouseWheel+=(o,e)=>{mp.Volume=Math.Max(0,Math.Min(1,mp.Volume+(e.Delta>0?.05:-.05)));sub.Text="Volume "+(int)Math.Round(mp.Volume*100)+"%";};
  AllowDrop=true; DragEnter+=(o,e)=>{e.Effect=DragDropEffects.Copy;}; DragDrop+=(o,e)=>{AddFiles((string[])e.Data.GetData(DataFormats.FileDrop));};
  mp.MediaEnded+=delegate{if(loop==2){mp.Position=TimeSpan.Zero;mp.Play();}else if(cur<files.Count-1||loop==1)Go(cur+1);else{playing=false;play.Text="\uE768";}};
  mp.MediaFailed+=delegate{sub.Text="Can't play this file";};
  t.Interval=250; t.Tick+=delegate{bar.Invalidate();if(cur>=0&&mp.NaturalDuration.HasTimeSpan&&!sub.Text.StartsWith("Volume"))sub.Text=F(mp.Position)+" / "+F(mp.NaturalDuration.TimeSpan)+"   \u2022   "+(cur+1)+" of "+files.Count;else if(sub.Text.StartsWith("Volume"))vt++; if(vt>6){vt=0;sub.Text="";}}; t.Start();
  mp.Volume=0.8; LoadState(); Paint2();
  FormClosing+=delegate{Save();};
 }
 int vt=0;
 void Fill(Graphics g,Col c,int x,int w){if(w<3)return;using(var p=new GraphicsPath()){p.AddArc(x,0,6,6,90,180);p.AddArc(x+w-7,0,6,6,270,180);p.CloseFigure();g.FillPath(new SolidBrush(c),p);}}
 double Frac(){return mp.NaturalDuration.HasTimeSpan&&mp.NaturalDuration.TimeSpan.TotalSeconds>0?mp.Position.TotalSeconds/mp.NaturalDuration.TimeSpan.TotalSeconds:0;}
 void Size(int h){ClientSize=new System.Drawing.Size(WD,h);Region=Region.FromHrgn(CreateRoundRectRgn(0,0,WD+1,h+1,18,18));if(h>H)Top=Math.Max(0,Top-(LH)) ;else Top+=0;}
 void Drag(object o,MouseEventArgs e){if(e.Button==MouseButtons.Left){ReleaseCapture();SendMessage(Handle,0xA1,2,0);}}
 void Paint2(){loopB.Text=loop==2?"\uE8ED":"\uE8EE";loopB.ForeColor=loop>0?acc:Col.White;}
 string F(TimeSpan s){return (int)s.TotalMinutes+":"+s.Seconds.ToString("00");}
 void Add(){var d=new OpenFileDialog();d.Multiselect=true;d.Filter="Audio & Video|*.mp3;*.mp4;*.m4a;*.m4b;*.aac;*.wav;*.wma;*.flac;*.alac;*.ogg;*.opus;*.oga;*.aiff;*.aif;*.ac3;*.amr;*.3gp;*.mka;*.webm;*.mkv;*.mov;*.wmv;*.avi|All files|*.*";if(d.ShowDialog()==DialogResult.OK)AddFiles(d.FileNames);}
 void AddFiles(string[] fs){bool first=cur<0;foreach(var f in fs)if(!files.Contains(f)){files.Add(f);list.Items.Add(f);}Save();if(first&&files.Count>0)Go(0);}
 void Go(int k){if(files.Count==0)return;cur=(k%files.Count+files.Count)%files.Count;mp.Open(new Uri(files[cur]));title.Text=Path.GetFileNameWithoutExtension(files[cur]);mp.Play();playing=true;play.Text="\uE769";list.Invalidate();Save();}
 void Toggle(){if(cur<0){if(files.Count>0)Go(0);return;}if(playing){mp.Pause();play.Text="\uE768";}else{mp.Play();play.Text="\uE769";}playing=!playing;}
 void Save(){try{Directory.CreateDirectory(store);var l=new List<string>();l.Add(cur+"|"+loop+"|"+(int)(mp.Volume*100));l.AddRange(files);File.WriteAllLines(Path.Combine(store,"playlist.txt"),l.ToArray());}catch{}}
 void LoadState(){try{var p=Path.Combine(store,"playlist.txt");if(!File.Exists(p))return;var l=File.ReadAllLines(p);var h=l[0].Split('|');loop=int.Parse(h[1]);mp.Volume=int.Parse(h[2])/100.0;
  for(int i=1;i<l.Length;i++)if(File.Exists(l[i])){files.Add(l[i]);list.Items.Add(l[i]);}
  int c=int.Parse(h[0]);if(files.Count>0){cur=Math.Max(0,Math.Min(c,files.Count-1));mp.Open(new Uri(files[cur]));title.Text=Path.GetFileNameWithoutExtension(files[cur]);sub.Text=files.Count+" songs saved";}}catch{}}
 [STAThread] static void Main(){Application.EnableVisualStyles();Application.Run(new W());}
}
